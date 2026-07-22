use std::{
    collections::{HashMap, HashSet},
    convert::Infallible,
    future::Future,
    ops::Deref,
    sync::Arc,
};

use phoenix_http::{BoxFuture, Handler, Middleware, Next, Request, Response, StatusCode};
use phoenix_routing::Routes;
use thiserror::Error;

pub use phoenix_dx_macros::mount_routes;

#[derive(Clone)]
struct SharedHandler(Arc<dyn Handler>);

impl Handler for SharedHandler {
    fn call(&self, request: Request) -> BoxFuture<Response> {
        self.0.call(request)
    }
}

#[derive(Clone)]
struct SharedMiddleware(Arc<dyn Middleware>);

impl Middleware for SharedMiddleware {
    fn handle(&self, request: Request, next: Next) -> BoxFuture<Response> {
        self.0.handle(request, next)
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ResourceAction {
    Index,
    Create,
    Store,
    Show,
    Edit,
    Update,
    Destroy,
}

impl ResourceAction {
    const ALL: [Self; 7] = [
        Self::Index,
        Self::Create,
        Self::Store,
        Self::Show,
        Self::Edit,
        Self::Update,
        Self::Destroy,
    ];
}

#[derive(Default)]
pub struct Resource {
    handlers: HashMap<ResourceAction, SharedHandler>,
    only: Option<HashSet<ResourceAction>>,
    except: HashSet<ResourceAction>,
    parameter: Option<String>,
}

macro_rules! resource_handler {
    ($method:ident, $action:ident) => {
        #[must_use]
        pub fn $method<H>(mut self, handler: H) -> Self
        where
            H: Handler,
        {
            self.handlers
                .insert(ResourceAction::$action, SharedHandler(Arc::new(handler)));
            self
        }
    };
}

impl Resource {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    resource_handler!(index, Index);
    resource_handler!(create, Create);
    resource_handler!(store, Store);
    resource_handler!(show, Show);
    resource_handler!(edit, Edit);
    resource_handler!(update, Update);
    resource_handler!(destroy, Destroy);

    #[must_use]
    pub fn only(mut self, actions: impl IntoIterator<Item = ResourceAction>) -> Self {
        self.only = Some(actions.into_iter().collect());
        self
    }

    #[must_use]
    pub fn except(mut self, actions: impl IntoIterator<Item = ResourceAction>) -> Self {
        self.except.extend(actions);
        self
    }

    #[must_use]
    pub fn parameter(mut self, parameter: impl Into<String>) -> Self {
        self.parameter = Some(parameter.into());
        self
    }

    fn enabled(&self, action: ResourceAction) -> bool {
        self.only.as_ref().is_none_or(|only| only.contains(&action))
            && !self.except.contains(&action)
    }
}

pub trait ResourceRoutes {
    #[must_use]
    fn resource(self, name: &str, path: &str, resource: Resource) -> Self;
}

impl ResourceRoutes for Routes {
    fn resource(mut self, name: &str, path: &str, mut resource: Resource) -> Self {
        let path = normalize_resource_path(path);
        let parameter = resource.parameter.take().unwrap_or_else(|| singular(name));
        let member = format!("{path}/{{{parameter}}}");

        for action in ResourceAction::ALL {
            if !resource.enabled(action) {
                continue;
            }
            let Some(handler) = resource.handlers.remove(&action) else {
                continue;
            };
            self = match action {
                ResourceAction::Index => self.get(&path, handler).name(format!("{name}.index")),
                ResourceAction::Create => self
                    .get(format!("{path}/create"), handler)
                    .name(format!("{name}.create")),
                ResourceAction::Store => self.post(&path, handler).name(format!("{name}.store")),
                ResourceAction::Show => self.get(&member, handler).name(format!("{name}.show")),
                ResourceAction::Edit => self
                    .get(format!("{member}/edit"), handler)
                    .name(format!("{name}.edit")),
                ResourceAction::Update => {
                    self = self
                        .put(&member, handler.clone())
                        .name(format!("{name}.update"));
                    self.patch(&member, handler)
                }
                ResourceAction::Destroy => self
                    .delete(&member, handler)
                    .name(format!("{name}.destroy")),
            };
        }
        self
    }
}

fn normalize_resource_path(path: &str) -> String {
    let path = path.trim();
    if path.is_empty() || path == "/" {
        String::new()
    } else {
        format!("/{}", path.trim_matches('/'))
    }
}

fn singular(name: &str) -> String {
    name.strip_suffix('s')
        .filter(|value| !value.is_empty())
        .unwrap_or(name)
        .to_owned()
}

#[derive(Clone, Default)]
pub struct MiddlewareAliases {
    aliases: HashMap<String, SharedMiddleware>,
}

impl MiddlewareAliases {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register<M>(&mut self, alias: impl Into<String>, middleware: M) -> &mut Self
    where
        M: Middleware,
    {
        self.aliases
            .insert(alias.into(), SharedMiddleware(Arc::new(middleware)));
        self
    }

    /// Apply aliases to the most recently declared route.
    ///
    /// # Errors
    ///
    /// Returns an error before router build when an alias is unknown.
    pub fn apply(
        &self,
        mut routes: Routes,
        aliases: &[&str],
    ) -> Result<Routes, MiddlewareAliasError> {
        for alias in aliases {
            let middleware = self
                .aliases
                .get(*alias)
                .cloned()
                .ok_or_else(|| MiddlewareAliasError((*alias).to_owned()))?;
            routes = routes.middleware(middleware);
        }
        Ok(routes)
    }
}

#[derive(Debug, Error, Eq, PartialEq)]
#[error("Phoenix middleware alias is not registered: {0}")]
pub struct MiddlewareAliasError(String);

#[derive(Debug)]
pub struct Bound<T>(Arc<T>);

impl<T> Clone for Bound<T> {
    fn clone(&self) -> Self {
        Self(Arc::clone(&self.0))
    }
}

impl<T> Bound<T> {
    #[must_use]
    pub fn from_request(request: &Request) -> Option<Self>
    where
        T: Send + Sync + 'static,
    {
        request.extensions().get::<Self>().cloned()
    }
}

impl<T> Deref for Bound<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

type ResolveFuture<T> = BoxFuture<Result<Option<T>, ModelBindingFailure>>;
type Resolver<T> = dyn Fn(String) -> ResolveFuture<T> + Send + Sync;

#[derive(Clone)]
pub struct ModelBinding<T> {
    parameter: String,
    resolver: Arc<Resolver<T>>,
}

impl<T> ModelBinding<T>
where
    T: Send + Sync + 'static,
{
    #[must_use]
    pub fn new<F, Fut, E>(parameter: impl Into<String>, resolver: F) -> Self
    where
        F: Fn(String) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<Option<T>, E>> + Send + 'static,
        E: Send + 'static,
    {
        let resolver = Arc::new(resolver);
        Self {
            parameter: parameter.into(),
            resolver: Arc::new(move |value| {
                let resolver = Arc::clone(&resolver);
                Box::pin(async move { resolver(value).await.map_err(|_| ModelBindingFailure) })
            }),
        }
    }
}

impl<T> Middleware for ModelBinding<T>
where
    T: Send + Sync + 'static,
{
    fn handle(&self, mut request: Request, next: Next) -> BoxFuture<Response> {
        let key = request.param(&self.parameter).map(str::to_owned);
        let resolver = Arc::clone(&self.resolver);
        Box::pin(async move {
            let Some(key) = key else {
                return Response::text("Not Found").with_status(StatusCode::NOT_FOUND);
            };
            match resolver(key).await {
                Ok(Some(model)) => {
                    request.extensions_mut().insert(Bound(Arc::new(model)));
                    next.run(request).await
                }
                Ok(None) => Response::text("Not Found").with_status(StatusCode::NOT_FOUND),
                Err(_) => Response::text("Internal Server Error")
                    .with_status(StatusCode::INTERNAL_SERVER_ERROR),
            }
        })
    }
}

#[derive(Debug)]
struct ModelBindingFailure;

impl From<Infallible> for ModelBindingFailure {
    fn from(value: Infallible) -> Self {
        match value {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use phoenix_http::{Method, Uri};
    use serde::Serialize;
    use std::future::{Ready, ready};

    fn ok(_request: Request) -> Ready<&'static str> {
        ready("ok")
    }

    #[tokio::test]
    async fn resource_routes_register_standard_methods_names_and_filters() {
        let router = Routes::new()
            .resource(
                "posts",
                "/posts",
                Resource::new()
                    .index(ok)
                    .store(ok)
                    .show(ok)
                    .update(ok)
                    .destroy(ok)
                    .except([ResourceAction::Destroy]),
            )
            .build()
            .unwrap();

        for (method, uri, name) in [
            (Method::GET, "/posts", "posts.index"),
            (Method::POST, "/posts", "posts.store"),
            (Method::GET, "/posts/7", "posts.show"),
            (Method::PUT, "/posts/7", "posts.update"),
            (Method::PATCH, "/posts/7", "posts.update"),
        ] {
            let response = router
                .handle(Request::new(method, uri.parse().unwrap()))
                .await;
            assert_eq!(response.status(), StatusCode::OK);
            let expected = if name == "posts.index" || name == "posts.store" {
                "/posts"
            } else {
                "/posts/7"
            };
            assert_eq!(router.url(name, &[("post", "7")]).unwrap(), expected);
        }
        assert_eq!(
            router
                .handle(Request::new(Method::DELETE, Uri::from_static("/posts/7")))
                .await
                .status(),
            StatusCode::METHOD_NOT_ALLOWED
        );
    }

    #[derive(Clone, Debug, Serialize)]
    struct User {
        id: u64,
    }

    #[tokio::test]
    async fn aliases_and_model_binding_fail_closed() {
        let mut aliases = MiddlewareAliases::new();
        aliases.register(
            "user",
            ModelBinding::new("user", |value| async move {
                Ok::<_, Infallible>((value == "7").then_some(User { id: 7 }))
            }),
        );
        let declarations = aliases
            .apply(
                Routes::new().get("/users/{user}", |request: Request| async move {
                    Bound::<User>::from_request(&request)
                        .map_or_else(|| "missing".to_owned(), |user| user.id.to_string())
                }),
                &["user"],
            )
            .unwrap();
        let router = declarations.build().unwrap();

        assert_eq!(
            router
                .handle(Request::new(Method::GET, Uri::from_static("/users/7")))
                .await
                .body(),
            "7"
        );
        assert_eq!(
            router
                .handle(Request::new(Method::GET, Uri::from_static("/users/8")))
                .await
                .status(),
            StatusCode::NOT_FOUND
        );
        assert!(matches!(
            aliases.apply(Routes::new().get("/", ok), &["missing"]),
            Err(MiddlewareAliasError(_))
        ));
    }
}
