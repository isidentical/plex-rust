//! Posters, backdrops and title logos.

use reqwest::Method;

use crate::error::Result;
use crate::server::MediaServer;

impl MediaServer {
    /// Fetches artwork as a JPEG that covers `width` by `height` pixels.
    ///
    /// `path` is a server path from an item, such as its `thumb`. The server
    /// scales the image, so only the pixels that will be shown are sent.
    pub async fn image(&self, path: &str, width: u32, height: u32) -> Result<Vec<u8>> {
        // Scale to cover the box, not to fit inside it.
        self.transcoded(path, width, height, &[("minSize", "1"), ("format", "jpg")]).await
    }

    /// Fetches artwork that is clear around its subject, such as a title
    /// logo, as a PNG that fits inside `width` by `height` pixels.
    ///
    /// A JPEG has no clear parts: the server paints them black. The whole
    /// of a logo has to be seen, so it is scaled to fit the box and not
    /// to cover it.
    pub async fn cutout(&self, path: &str, width: u32, height: u32) -> Result<Vec<u8>> {
        self.transcoded(path, width, height, &[("format", "png")]).await
    }

    async fn transcoded(
        &self,
        path: &str,
        width: u32,
        height: u32,
        how: &[(&str, &str)],
    ) -> Result<Vec<u8>> {
        let size = [("url", path), ("width", &width.to_string()), ("height", &height.to_string())];
        let request = self.request(Method::GET, "/photo/:/transcode").query(&size).query(how);
        Ok(request.send().await?.error_for_status()?.bytes().await?.to_vec())
    }
}
