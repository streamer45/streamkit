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
