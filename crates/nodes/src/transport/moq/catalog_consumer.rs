// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

/// Receives catalog updates from a MoQ track.
///
/// Vendored from hang 0.15.x; upstream removed this in hang 0.16.
#[derive(Clone)]
pub struct CatalogConsumer {
    pub track: moq_lite::TrackConsumer,
    group: Option<moq_lite::GroupConsumer>,
}

impl CatalogConsumer {
    pub const fn new(track: moq_lite::TrackConsumer) -> Self {
        Self { track, group: None }
    }

    pub async fn next(&mut self) -> Result<Option<hang::catalog::Catalog>, hang::Error> {
        loop {
            tokio::select! {
                res = self.track.next_group() => {
                    match res? {
                        Some(group) => {
                            self.group = Some(group);
                        }
                        None => return Ok(None),
                    }
                },
                Some(frame) = async { self.group.as_mut()?.read_frame().await.transpose() } => {
                    self.group.take();
                    let catalog = hang::catalog::Catalog::from_slice(&frame?)?;
                    return Ok(Some(catalog));
                }
            }
        }
    }
}

impl From<moq_lite::TrackConsumer> for CatalogConsumer {
    fn from(inner: moq_lite::TrackConsumer) -> Self {
        Self::new(inner)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn make_track_pair() -> (moq_lite::TrackProducer, moq_lite::TrackConsumer) {
        let origin = moq_lite::Origin::random().produce();
        let mut broadcast = origin.create_broadcast("test-broadcast").expect("create_broadcast");
        let track = moq_lite::Track { name: ".catalog".to_string(), priority: 0 };
        let producer = broadcast.create_track(track.clone()).expect("create_track");
        let consumer = origin.consume();
        let bc = consumer.get_broadcast("test-broadcast").expect("get_broadcast");
        let consumer_track = bc.subscribe_track(&track).expect("subscribe_track");
        (producer, consumer_track)
    }

    #[tokio::test]
    async fn new_initialises_empty_state() {
        let (_producer, consumer) = make_track_pair();
        let cc = CatalogConsumer::new(consumer);
        assert!(cc.group.is_none());
    }

    #[tokio::test]
    async fn from_trait_constructs_consumer() {
        let (_producer, consumer) = make_track_pair();
        let cc = CatalogConsumer::from(consumer);
        assert!(cc.group.is_none());
    }

    #[tokio::test]
    async fn next_returns_none_when_track_finished() {
        let (mut producer, consumer) = make_track_pair();
        let mut cc = CatalogConsumer::new(consumer);
        producer.finish().expect("finish");
        let result = cc.next().await.expect("next should not error");
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn next_returns_catalog_from_written_frame() {
        let (mut producer, consumer) = make_track_pair();
        let mut cc = CatalogConsumer::new(consumer);

        let catalog = hang::catalog::Catalog::default();
        let payload = serde_json::to_vec(&catalog).expect("serialize catalog");

        let mut group = producer.append_group().expect("append_group");
        group.write_frame(bytes::Bytes::from(payload)).expect("write_frame");
        group.finish().expect("finish group");

        let result = cc.next().await.expect("next should not error");
        assert!(result.is_some(), "should have received a catalog");
    }

    fn write_catalog(producer: &mut moq_lite::TrackProducer, catalog: &hang::catalog::Catalog) {
        let payload = serde_json::to_vec(catalog).expect("serialize catalog");
        let mut group = producer.append_group().expect("append_group");
        group.write_frame(bytes::Bytes::from(payload)).expect("write_frame");
        group.finish().expect("finish group");
    }

    /// `MoqPullNode::run_connection` polls `next()` as one arm of a `select!`
    /// that media frames win tens of times per second, so the future is created
    /// and dropped repeatedly. This exercises that contention directly: poll and
    /// drop `next()` ~100× with no new data, then confirm a late catalog written
    /// afterwards is still observed (no buffered position is lost on cancel).
    #[tokio::test]
    async fn next_survives_repeated_cancellation() {
        let (mut producer, consumer) = make_track_pair();
        let mut cc = CatalogConsumer::new(consumer);

        write_catalog(&mut producer, &hang::catalog::Catalog::default());
        assert!(cc.next().await.expect("first next").is_some(), "first catalog");

        for _ in 0..100 {
            tokio::select! {
                biased;
                _ = cc.next() => panic!("no catalog update should be ready yet"),
                () = tokio::task::yield_now() => {},
            }
        }

        let mut updated = hang::catalog::Catalog::default();
        updated.audio.renditions.insert("audio/data".to_string(), {
            let mut cfg = hang::catalog::AudioConfig::new(
                super::super::constants::catalog_audio_codec(
                    streamkit_core::types::AudioCodec::Opus,
                ),
                48000,
                2,
            );
            cfg.bitrate = Some(128_000);
            cfg
        });
        write_catalog(&mut producer, &updated);

        let result = cc.next().await.expect("late next").expect("late catalog after cancellation");
        assert_eq!(
            result.audio.renditions.len(),
            1,
            "late catalog update written during contention must still be delivered"
        );
    }
}
