// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

/// Receives catalog updates from a MoQ track.
///
/// Vendored from hang 0.15.x; upstream removed this in hang 0.16.
pub struct CatalogConsumer {
    pub track: moq_net::track::Subscriber,
    group: Option<moq_net::group::Consumer>,
}

impl CatalogConsumer {
    pub const fn new(track: moq_net::track::Subscriber) -> Self {
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
                    let catalog = hang::catalog::Catalog::from_slice(&frame?.payload)?;
                    return Ok(Some(catalog));
                }
            }
        }
    }
}

impl From<moq_net::track::Subscriber> for CatalogConsumer {
    fn from(inner: moq_net::track::Subscriber) -> Self {
        Self::new(inner)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    async fn make_track_pair() -> (
        moq_net::origin::Producer,
        moq_net::broadcast::Producer,
        moq_net::track::Producer,
        moq_net::track::Subscriber,
    ) {
        let origin = moq_net::Origin::random().produce();
        let mut broadcast = origin
            .create_broadcast("test-broadcast", moq_net::broadcast::Route::announced())
            .expect("create_broadcast");
        let producer = super::super::create_catalog_track(&mut broadcast).expect("create_track");
        let consumer = origin.consume();
        let bc = consumer.announced_broadcast("test-broadcast").await.expect("announced_broadcast");
        let consumer_track = super::super::subscribe_catalog(&bc).await.expect("subscribe");
        (origin, broadcast, producer, consumer_track)
    }

    #[tokio::test]
    async fn new_initialises_empty_state() {
        let (_origin, _broadcast, _producer, consumer) = make_track_pair().await;
        let cc = CatalogConsumer::new(consumer);
        assert!(cc.group.is_none());
    }

    #[tokio::test]
    async fn from_trait_constructs_consumer() {
        let (_origin, _broadcast, _producer, consumer) = make_track_pair().await;
        let cc = CatalogConsumer::from(consumer);
        assert!(cc.group.is_none());
    }

    #[tokio::test]
    async fn next_returns_none_when_track_finished() {
        let (_origin, _broadcast, mut producer, consumer) = make_track_pair().await;
        let mut cc = CatalogConsumer::new(consumer);
        producer.finish().expect("finish");
        let result = cc.next().await.expect("next should not error");
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn next_returns_catalog_from_written_frame() {
        let (_origin, _broadcast, mut producer, consumer) = make_track_pair().await;
        let mut cc = CatalogConsumer::new(consumer);

        let catalog = hang::catalog::Catalog::default();
        write_catalog(&mut producer, &catalog);

        let result = cc.next().await.expect("next should not error");
        assert!(result.is_some(), "should have received a catalog");
    }

    fn write_catalog(producer: &mut moq_net::track::Producer, catalog: &hang::catalog::Catalog) {
        let payload = catalog.to_vec().expect("serialize catalog");
        super::super::write_catalog_json(producer, payload).expect("write catalog");
    }

    /// Dynamic track add/remove republishes the catalog repeatedly; every
    /// snapshot lands in its own group, so a consumer keeping pace observes
    /// each successive update.
    #[tokio::test]
    async fn next_observes_each_republished_snapshot() {
        let (_origin, _broadcast, mut producer, consumer) = make_track_pair().await;
        let mut cc = CatalogConsumer::new(consumer);

        for count in 1..=3usize {
            let mut catalog = hang::catalog::Catalog::default();
            for i in 0..count {
                catalog.audio.renditions.insert(
                    format!("audio/{i}"),
                    hang::catalog::AudioConfig::new(
                        super::super::constants::catalog_audio_codec(
                            streamkit_core::types::AudioCodec::Opus,
                        ),
                        48000,
                        2,
                    ),
                );
            }
            write_catalog(&mut producer, &catalog);

            let result = cc.next().await.expect("next").expect("catalog snapshot");
            assert_eq!(result.audio.renditions.len(), count, "republish {count} not observed");
        }
    }

    /// `MoqPullNode::run_connection` polls `next()` as one arm of a `select!`
    /// that media frames win tens of times per second, so the future is created
    /// and dropped repeatedly. This exercises that contention directly: poll and
    /// drop `next()` ~100× with no new data, then confirm a late catalog written
    /// afterwards is still observed (no buffered position is lost on cancel).
    #[tokio::test]
    async fn next_survives_repeated_cancellation() {
        let (_origin, _broadcast, mut producer, consumer) = make_track_pair().await;
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
