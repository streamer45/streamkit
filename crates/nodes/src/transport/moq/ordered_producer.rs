// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

use hang::container::{Frame, Timestamp};

/// Outcome of [`OrderedProducer::write_video`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VideoWrite {
    /// The frame was encoded into a (possibly new) group.
    Written,
    /// A leading delta frame was dropped because no keyframe-led group is open
    /// yet. The caller should advance its media clock so audio/video timing
    /// stays aligned, but must not count the frame as sent.
    DroppedLeadingDelta,
}

/// Wraps a `moq_lite::TrackProducer` with MoQ group boundary management.
///
/// Groups can be managed explicitly via [`keyframe()`](Self::keyframe) or automatically
/// via [`with_max_group_duration()`](Self::with_max_group_duration).
///
/// Vendored from hang 0.15.x; upstream moved this into `moq-mux` in hang 0.16
/// which pulls heavy deps (mp4-atom, h264-parser, m3u8-rs) that StreamKit does
/// not need.
#[derive(Clone)]
#[allow(dead_code)] // vendored API surface retained for parity with upstream hang
pub struct OrderedProducer {
    pub track: moq_lite::TrackProducer,
    group: Option<moq_lite::GroupProducer>,
    group_start: Option<Timestamp>,
    group_frames: u64,
    max_group_duration: Option<Timestamp>,
}

#[allow(dead_code)] // vendored API surface retained for parity with upstream hang
impl OrderedProducer {
    pub const fn new(inner: moq_lite::TrackProducer) -> Self {
        Self {
            track: inner,
            group: None,
            group_start: None,
            group_frames: 0,
            max_group_duration: None,
        }
    }

    pub const fn with_max_group_duration(mut self, duration: Timestamp) -> Self {
        self.max_group_duration = Some(duration);
        self
    }

    /// Close the current group so the next `write()` starts a fresh one.
    ///
    /// Despite the name this only finishes the open group (it does not itself
    /// write a keyframe); video callers should prefer [`write_video`](Self::write_video),
    /// which owns the "a group never opens on a delta" invariant.
    pub fn keyframe(&mut self) -> Result<(), hang::Error> {
        if let Some(mut group) = self.group.take() {
            group.finish()?;
        }
        Ok(())
    }

    /// Write a video frame while guaranteeing a MoQ group never *begins* on a
    /// delta frame.
    ///
    /// A keyframe finishes the current group and opens a fresh one (so every
    /// group starts decodable). A delta is appended to the open group, or
    /// **dropped** when no group is open yet — a group that opens on a delta
    /// wedges a late-joining subscriber's `VideoDecoder` ("a key frame is
    /// required after configure()"). Because `keyframe()` is the only thing
    /// that closes a group and it is immediately followed by the keyframe's
    /// `write()`, the only moment a group is absent is before the first
    /// keyframe (or after a fresh producer is created), so this gate is
    /// self-arming and needs no external bookkeeping.
    ///
    /// Returns [`VideoWrite::DroppedLeadingDelta`] when the frame was dropped so
    /// the caller can still advance its media clock (keeping audio/video timing
    /// aligned) without counting the frame as sent.
    pub fn write_video(
        &mut self,
        frame: &Frame,
        is_keyframe: bool,
    ) -> Result<VideoWrite, hang::Error> {
        if is_keyframe {
            self.keyframe()?;
            self.write(frame)?;
            Ok(VideoWrite::Written)
        } else if self.group.is_some() {
            self.write(frame)?;
            Ok(VideoWrite::Written)
        } else {
            Ok(VideoWrite::DroppedLeadingDelta)
        }
    }

    /// Encode a [`Frame`] into the current MoQ group, creating a new group when
    /// needed (first frame, after `keyframe()`, or when `max_group_duration` is
    /// exceeded).
    pub fn write(&mut self, frame: &Frame) -> Result<(), hang::Error> {
        tracing::trace!(?frame, "write frame");

        if let (Some(max_duration), Some(group_start)) = (self.max_group_duration, self.group_start)
        {
            if self.group.is_some()
                && frame.timestamp.checked_sub(group_start).unwrap_or(Timestamp::ZERO)
                    >= max_duration
            {
                if let Some(mut group) = self.group.take() {
                    group.finish()?;
                }
            }
        }

        if self.group.is_none() {
            let group = self.track.append_group()?;
            self.group = Some(group);
            self.group_start = Some(frame.timestamp);
            self.group_frames = 0;
        }

        #[allow(clippy::unwrap_used)] // is_none branch above guarantees Some
        let mut group = self.group.take().unwrap();
        frame.encode(&mut group)?;
        self.group.replace(group);

        self.group_frames += 1;

        // Estimate the next frame's timestamp and close the group now if it
        // would exceed the limit.
        if let (Some(max_duration), Some(group_start)) = (self.max_group_duration, self.group_start)
        {
            let elapsed =
                frame.timestamp.checked_sub(group_start).unwrap_or(Timestamp::ZERO).as_micros();
            let max = max_duration.as_micros();

            if elapsed * (u128::from(self.group_frames) + 1) >= max * u128::from(self.group_frames)
            {
                if let Some(mut group) = self.group.take() {
                    group.finish()?;
                }
            }
        }

        Ok(())
    }

    pub fn finish(&mut self) -> Result<(), hang::Error> {
        if let Some(mut group) = self.group.take() {
            group.finish()?;
        }
        self.track.finish()?;
        Ok(())
    }
}

impl From<moq_lite::TrackProducer> for OrderedProducer {
    fn from(inner: moq_lite::TrackProducer) -> Self {
        Self::new(inner)
    }
}

impl std::ops::Deref for OrderedProducer {
    type Target = moq_lite::TrackProducer;

    fn deref(&self) -> &Self::Target {
        &self.track
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn make_track_producer() -> moq_lite::TrackProducer {
        let origin = moq_lite::Origin::random().produce();
        let mut broadcast =
            origin.create_broadcast("test-broadcast").expect("create_broadcast should succeed");
        broadcast
            .create_track(moq_lite::Track { name: "test/track".to_string(), priority: 0 })
            .expect("create_track should succeed")
    }

    fn ts(micros: u64) -> Timestamp {
        Timestamp::from_micros(micros).unwrap()
    }

    fn make_frame(ts_micros: u64) -> Frame {
        Frame { timestamp: ts(ts_micros), payload: bytes::Bytes::from_static(b"test-payload") }
    }

    #[tokio::test]
    async fn new_initialises_empty_state() {
        let tp = make_track_producer();
        let op = OrderedProducer::new(tp);
        assert!(op.group.is_none());
        assert!(op.group_start.is_none());
        assert_eq!(op.group_frames, 0);
        assert!(op.max_group_duration.is_none());
    }

    #[tokio::test]
    async fn with_max_group_duration_sets_limit() {
        let tp = make_track_producer();
        let op = OrderedProducer::new(tp).with_max_group_duration(ts(1_000_000));
        assert_eq!(op.max_group_duration, Some(ts(1_000_000)));
    }

    #[tokio::test]
    async fn from_trait_constructs_producer() {
        let tp = make_track_producer();
        let op = OrderedProducer::from(tp);
        assert!(op.group.is_none());
    }

    #[tokio::test]
    async fn deref_exposes_inner_track() {
        let tp = make_track_producer();
        let op = OrderedProducer::new(tp);
        let _: &moq_lite::TrackProducer = &op;
    }

    #[tokio::test]
    async fn write_creates_group_on_first_frame() {
        let tp = make_track_producer();
        let mut op = OrderedProducer::new(tp);
        assert!(op.group.is_none());
        op.write(&make_frame(0)).expect("write should succeed");
        assert!(op.group.is_some());
        assert_eq!(op.group_frames, 1);
        assert_eq!(op.group_start, Some(ts(0)));
    }

    #[tokio::test]
    async fn write_multiple_frames_increments_counter() {
        let tp = make_track_producer();
        let mut op = OrderedProducer::new(tp);
        op.write(&make_frame(0)).unwrap();
        op.write(&make_frame(1000)).unwrap();
        op.write(&make_frame(2000)).unwrap();
        assert_eq!(op.group_frames, 3);
    }

    #[tokio::test]
    async fn keyframe_on_empty_is_noop() {
        let tp = make_track_producer();
        let mut op = OrderedProducer::new(tp);
        op.keyframe().expect("keyframe on empty should succeed");
        assert!(op.group.is_none());
    }

    #[tokio::test]
    async fn keyframe_closes_active_group() {
        let tp = make_track_producer();
        let mut op = OrderedProducer::new(tp);
        op.write(&make_frame(0)).unwrap();
        assert!(op.group.is_some());
        op.keyframe().expect("keyframe should succeed");
        assert!(op.group.is_none());
    }

    #[tokio::test]
    async fn keyframe_then_write_starts_new_group() {
        let tp = make_track_producer();
        let mut op = OrderedProducer::new(tp);
        op.write(&make_frame(0)).unwrap();
        assert_eq!(op.group_start, Some(ts(0)));
        op.keyframe().unwrap();
        op.write(&make_frame(5000)).unwrap();
        assert_eq!(op.group_start, Some(ts(5000)));
        assert_eq!(op.group_frames, 1);
    }

    #[tokio::test]
    async fn max_duration_auto_closes_old_group_and_starts_new() {
        let tp = make_track_producer();
        let mut op = OrderedProducer::new(tp).with_max_group_duration(ts(10_000));

        op.write(&make_frame(0)).unwrap();
        assert!(op.group.is_some());
        assert_eq!(op.group_start, Some(ts(0)));

        op.write(&make_frame(10_000)).unwrap();
        assert!(op.group.is_some());
        assert_eq!(op.group_start, Some(ts(10_000)));
        assert_eq!(op.group_frames, 1);
    }

    #[tokio::test]
    async fn finish_closes_group_and_track() {
        let tp = make_track_producer();
        let mut op = OrderedProducer::new(tp);
        op.write(&make_frame(0)).unwrap();
        op.finish().expect("finish should succeed");
        assert!(op.group.is_none());
    }

    #[tokio::test]
    async fn finish_on_empty_finishes_track() {
        let tp = make_track_producer();
        let mut op = OrderedProducer::new(tp);
        op.finish().expect("finish on empty should succeed");
    }

    #[tokio::test]
    async fn write_video_drops_leading_deltas_until_first_keyframe() {
        let tp = make_track_producer();
        let mut op = OrderedProducer::new(tp);

        // Deltas before any keyframe must not open a group: a group that
        // begins on a delta wedges a late-joining decoder.
        assert_eq!(op.write_video(&make_frame(0), false).unwrap(), VideoWrite::DroppedLeadingDelta);
        assert!(op.group.is_none(), "no group is opened by a leading delta");
        assert_eq!(
            op.write_video(&make_frame(1000), false).unwrap(),
            VideoWrite::DroppedLeadingDelta
        );
        assert!(op.group.is_none());
    }

    #[tokio::test]
    async fn write_video_opens_group_on_keyframe_and_appends_deltas() {
        let tp = make_track_producer();
        let mut op = OrderedProducer::new(tp);

        // First keyframe opens group 0.
        assert_eq!(op.write_video(&make_frame(0), true).unwrap(), VideoWrite::Written);
        assert!(op.group.is_some());
        assert_eq!(op.group_frames, 1);
        assert_eq!(op.group_start, Some(ts(0)));

        // A delta after a keyframe is appended to the open group.
        assert_eq!(op.write_video(&make_frame(1000), false).unwrap(), VideoWrite::Written);
        assert_eq!(op.group_frames, 2);
        assert_eq!(op.group_start, Some(ts(0)), "still the same keyframe-led group");

        // A subsequent keyframe closes the current group and opens a fresh one.
        assert_eq!(op.write_video(&make_frame(5000), true).unwrap(), VideoWrite::Written);
        assert_eq!(op.group_frames, 1);
        assert_eq!(op.group_start, Some(ts(5000)), "new group starts on the keyframe");
    }
}
