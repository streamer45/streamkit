// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

use hang::container::{Frame, Timestamp};

/// Wraps a `moq_lite::TrackProducer` with MoQ group boundary management.
///
/// Groups can be managed explicitly via [`keyframe()`](Self::keyframe) or automatically
/// via [`with_max_group_duration()`](Self::with_max_group_duration).
///
/// Vendored from hang 0.15.x; upstream moved this into `moq-mux` in hang 0.16
/// which pulls heavy deps (mp4-atom, h264-parser, m3u8-rs) that StreamKit does
/// not need.
#[derive(Clone)]
#[allow(dead_code)]
pub struct OrderedProducer {
    pub track: moq_lite::TrackProducer,
    group: Option<moq_lite::GroupProducer>,
    group_start: Option<Timestamp>,
    group_frames: u64,
    max_group_duration: Option<Timestamp>,
}

#[allow(dead_code)]
impl OrderedProducer {
    pub fn new(inner: moq_lite::TrackProducer) -> Self {
        Self {
            track: inner,
            group: None,
            group_start: None,
            group_frames: 0,
            max_group_duration: None,
        }
    }

    pub fn with_max_group_duration(mut self, duration: Timestamp) -> Self {
        self.max_group_duration = Some(duration);
        self
    }

    /// Close the current group so the next `write()` starts a fresh one.
    pub fn keyframe(&mut self) -> Result<(), hang::Error> {
        if let Some(mut group) = self.group.take() {
            group.finish()?;
        }
        Ok(())
    }

    /// Encode a [`Frame`] into the current MoQ group, creating a new group when
    /// needed (first frame, after `keyframe()`, or when `max_group_duration` is
    /// exceeded).
    pub fn write(&mut self, frame: Frame) -> Result<(), hang::Error> {
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

        let mut group = self.group.take().expect("group should exist");
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

            if elapsed * (self.group_frames as u128 + 1) >= max * self.group_frames as u128 {
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
