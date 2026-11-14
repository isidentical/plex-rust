//! Posters and backdrops.

use reqwest::Method;

use crate::error::Result;
use crate::server::MediaServer;

impl MediaServer {
    /// Fetches artwork as a JPEG that covers `width` by `height` pixels.
    ///
    /// `path` is a server path from an item, such as its `thumb`. The server
    /// scales the image, so only the pixels that will be shown are sent.
    pub async fn image(&self, path: &str, width: u32, height: u32) -> Result<Vec<u8>> {
        let response = self
            .request(Method::GET, "/photo/:/transcode")
            .query(&[
                ("url", path),
                ("width", &width.to_string()),
                ("height", &height.to_string()),
                // Scale to cover the box, not to fit inside it.
                ("minSize", "1"),
                ("format", "jpg"),
            ])
            .send()
            .await?;
        Ok(response.error_for_status()?.bytes().await?.to_vec())
    }
}
