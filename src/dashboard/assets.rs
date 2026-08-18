use axum::body::Body;
use axum::http::{header, HeaderValue, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use rust_embed::RustEmbed;

const IMMUTABLE_CACHE: &str = "public, max-age=31536000, immutable";

#[derive(RustEmbed)]
#[folder = "dashboard/dist/"]
struct DashboardAssets;

pub(super) async fn static_asset(uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');
    let path = if path.is_empty() {
        "index.html".to_string()
    } else if path.ends_with('/') {
        format!("{path}index.html")
    } else {
        path.to_string()
    };
    let Some(asset) = DashboardAssets::get(&path) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let mime = mime_guess::from_path(&path).first_or_octet_stream();
    let mut response = Response::new(Body::from(asset.data));
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_str(mime.as_ref())
            .unwrap_or(HeaderValue::from_static("application/octet-stream")),
    );
    let cache = if path.starts_with("assets/") {
        HeaderValue::from_static(IMMUTABLE_CACHE)
    } else {
        HeaderValue::from_static("no-cache, must-revalidate")
    };
    response.headers_mut().insert(header::CACHE_CONTROL, cache);
    response
}

pub(super) fn javascript_response(body: String) -> Response {
    let mut response = Response::new(Body::from(body));
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/javascript; charset=utf-8"),
    );
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundle_contains_dashboard() {
        let path = "index.html";
        assert!(
            DashboardAssets::get(path).is_some(),
            "dashboard bundle is missing {path}"
        );
        for obsolete in [
            "design-system.html",
            "execution.html",
            "compare.html",
            "coverage/index.html",
        ] {
            assert!(
                DashboardAssets::get(obsolete).is_none(),
                "dashboard bundle still contains obsolete page {obsolete}"
            );
        }
        assert!(
            DashboardAssets::iter().any(|path| path.starts_with("assets/")),
            "dashboard bundle is missing hashed Vite assets"
        );
    }
}
