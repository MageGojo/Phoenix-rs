use phoenix::prelude::{Page, Request, Response};

use crate::controllers::respond_with_renderer;
use crate::props::HomeProps;

pub struct HomeController;

impl HomeController {
    pub async fn index(request: Request) -> Response {
        respond_with_renderer(
            request,
            Page::new(
                "home",
                HomeProps {
                    title: "Phoenix render modes".to_owned(),
                    description: "One Rust application demonstrates three React rendering modes."
                        .to_owned(),
                },
            )
            .islands(),
        )
        .await
    }
}
