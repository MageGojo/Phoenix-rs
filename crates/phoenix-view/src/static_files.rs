use std::{
    fs,
    path::{Path, PathBuf},
    sync::Arc,
};

use phoenix_http::{
    BoxFuture, Bytes, HeaderValue, Middleware, Next, Request, Response, StatusCode, header,
};

use crate::assets::AssetManifest;

/// Serve Vite production assets declared by [`AssetManifest`].
///
/// Requests whose path starts with the manifest `public_path` (default `/assets/`)
/// are resolved through the manifest and read from `root` (typically `public/assets`).
/// All other requests continue down the middleware stack.
#[derive(Clone, Debug)]
pub struct ServeProductionAssets {
    assets: Arc<AssetManifest>,
    root: PathBuf,
}

impl ServeProductionAssets {
    #[must_use]
    pub fn new(assets: AssetManifest, root: impl Into<PathBuf>) -> Self {
        Self {
            assets: Arc::new(assets),
            root: root.into(),
        }
    }
}

impl Middleware for ServeProductionAssets {
    fn handle(&self, request: Request, next: Next) -> BoxFuture<Response> {
        let assets = Arc::clone(&self.assets);
        let root = self.root.clone();
        Box::pin(async move {
            let path = request.uri().path();
            if !path.starts_with(assets.public_path.as_str()) {
                return next.run(request).await;
            }
            match assets.resolve_static(&root, path) {
                Ok(file) => file_response(&file),
                Err(_) => Response::text("Not Found").with_status(StatusCode::NOT_FOUND),
            }
        })
    }
}

fn file_response(path: &Path) -> Response {
    let Ok(bytes) = fs::read(path) else {
        return Response::text("Not Found").with_status(StatusCode::NOT_FOUND);
    };
    let mut response = Response::new(StatusCode::OK, Bytes::from(bytes));
    if let Some(content_type) = content_type_for(path)
        && let Ok(value) = HeaderValue::from_str(content_type)
    {
        response.headers_mut().insert(header::CONTENT_TYPE, value);
    }
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("public, max-age=31536000, immutable"),
    );
    response
}

fn content_type_for(path: &Path) -> Option<&'static str> {
    match path.extension().and_then(|value| value.to_str())? {
        "js" | "mjs" => Some("text/javascript; charset=utf-8"),
        "css" => Some("text/css; charset=utf-8"),
        "json" => Some("application/json; charset=utf-8"),
        "map" => Some("application/json; charset=utf-8"),
        "svg" => Some("image/svg+xml"),
        "png" => Some("image/png"),
        "jpg" | "jpeg" => Some("image/jpeg"),
        "webp" => Some("image/webp"),
        "woff2" => Some("font/woff2"),
        "woff" => Some("font/woff"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn javascript_content_type() {
        assert_eq!(
            content_type_for(Path::new("phoenix-abc.js")),
            Some("text/javascript; charset=utf-8")
        );
        assert_eq!(
            content_type_for(Path::new("client.css")),
            Some("text/css; charset=utf-8")
        );
    }
}
