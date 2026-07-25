use phoenix::prelude::{Page, Request, Response};

use crate::controllers::respond_with_renderer;
use crate::props::{IslandsProps, SpaProps, SsrProps};

pub struct RenderModesController;

impl RenderModesController {
    pub async fn spa(request: Request) -> Response {
        respond_with_renderer(
            request,
            Page::new(
                "spa",
                SpaProps {
                    title: "SPA page is ready".to_owned(),
                },
            )
            .spa(),
        )
        .await
    }

    pub async fn islands(request: Request) -> Response {
        respond_with_renderer(
            request,
            Page::new(
                "islands",
                IslandsProps {
                    title: "Islands page is ready".to_owned(),
                },
            )
            .islands(),
        )
        .await
    }

    pub async fn ssr(request: Request) -> Response {
        respond_with_renderer(
            request,
            Page::new(
                "ssr",
                SsrProps {
                    title: "SSR page is ready".to_owned(),
                },
            )
            .ssr(),
        )
        .await
    }
}
