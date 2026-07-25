// <phoenix:modules>
pub mod features_controller;
pub mod home_controller;
pub mod note_controller;
pub mod render_modes_controller;
pub use features_controller::FeaturesController;
pub use home_controller::HomeController;
pub use note_controller::NoteController;
pub use render_modes_controller::RenderModesController;
// </phoenix:modules>

use phoenix::prelude::{AssetManifest, NodeRenderer, Page, Request, Response, StatusCode};

use crate::config::AppConfig;

pub(crate) async fn respond_with_renderer(request: Request, mut page: Page) -> Response {
    let renderer = request.extensions().get::<NodeRenderer>().cloned();
    let assets = request
        .extensions()
        .get::<Option<AssetManifest>>()
        .and_then(Option::as_ref);
    let vite_dev_url = request
        .extensions()
        .get::<AppConfig>()
        .and_then(AppConfig::vite_dev_url);

    if let Some(assets) = assets {
        page = match page.production_assets(assets, "client") {
            Ok(page) => page,
            Err(error) => {
                return Response::text(format!("asset manifest error: {error}"))
                    .with_status(StatusCode::INTERNAL_SERVER_ERROR);
            }
        };
    } else if let Some(vite_dev_url) = vite_dev_url {
        page = page.script_src(format!(
            "{}/@id/__x00__virtual:phoenix/client",
            vite_dev_url.trim_end_matches('/'),
        ));
    }

    match renderer {
        Some(renderer) => page.respond_with_renderer(&request, &renderer).await,
        None => Response::text("Phoenix renderer is unavailable")
            .with_status(StatusCode::INTERNAL_SERVER_ERROR),
    }
}
