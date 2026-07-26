//! Batch test data: factories, a deterministic faker, and seeding.
//!
//! **Development and test only.** Two gates keep it that way, because either
//! one alone is a bad bet:
//!
//! 1. the `factory` Cargo feature — nothing here is compiled into a build that
//!    does not ask for it;
//! 2. [`Seeder::run`] refuses to write when the environment says production,
//!    because a feature flag is one careless `--all-features` away from being
//!    on in a release binary.
//!
//! # Usage
//!
//! ```ignore
//! phoenix::factory! {
//!     User, |f| User::create()
//!         .name(f.name())
//!         .email(f.unique_email()),
//! }
//!
//! // 带一个参数的工厂：用来接父模型的外键
//! phoenix::factory! {
//!     Post, |f, user_id: i64| Post::create()
//!         .title(f.sentence(6))
//!         .body(f.paragraph(3))
//!         .user_id(user_id),
//! }
//!
//! let mut seeder = Seeder::new(&mut db)?;
//! let users = seeder.create::<User>(10).await?;
//! for user in &users {
//!     seeder.create_with::<Post, _>(5, user.id).await?;
//! }
//! ```

use std::future::Future;
use std::pin::Pin;

use thiserror::Error;

use crate::Database;

mod faker;

pub use faker::{Faker, Locale};

/// Environment variables consulted before seeding, in order.
const ENVIRONMENT_VARIABLES: [&str; 3] = ["PHOENIX_ENV", "APP_ENV", "RUST_ENV"];

/// Environment names that refuse seeding.
const PROTECTED_ENVIRONMENTS: [&str; 3] = ["production", "prod", "staging"];

/// Why seeding did not happen.
#[derive(Debug, Error)]
pub enum FactoryError {
    /// The environment is one seeding refuses to touch.
    #[error(
        "refusing to seed in `{environment}`: factories are development-only. \
         Set PHOENIX_ENV to a development value, or call Seeder::forced() if \
         you genuinely mean to write generated rows here."
    )]
    ProtectedEnvironment {
        /// The environment name that was found.
        environment: String,
    },
    /// The database rejected an insert.
    #[error("factory insert failed: {0}")]
    Insert(String),
}

/// A model that can generate its own rows.
///
/// Implement with the [`factory!`](crate::factory) macro rather than by hand;
/// it exists as a trait so seeding can be generic.
pub trait Factory: Sized + Send {
    /// Insert one generated row.
    fn create_one<'a>(
        database: &'a mut Database,
        faker: &'a mut Faker,
    ) -> Pin<Box<dyn Future<Output = Result<Self, FactoryError>> + Send + 'a>>;
}

/// A model whose factory needs one argument — typically a parent's key.
pub trait FactoryWith<A>: Sized + Send {
    /// Insert one generated row built around `argument`.
    fn create_one_with<'a>(
        database: &'a mut Database,
        faker: &'a mut Faker,
        argument: A,
    ) -> Pin<Box<dyn Future<Output = Result<Self, FactoryError>> + Send + 'a>>;
}

/// Runs factories against a database.
///
/// Construct with [`Seeder::new`], which performs the environment check once,
/// up front — so a seeding run either refuses immediately or is safe to
/// complete, rather than aborting halfway with rows already written.
pub struct Seeder<'a> {
    database: &'a mut Database,
    faker: Faker,
}

impl std::fmt::Debug for Seeder<'_> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Seeder")
            .field("backend", &self.database.backend())
            .finish_non_exhaustive()
    }
}

impl<'a> Seeder<'a> {
    /// Bind a seeder to `database`, refusing protected environments.
    ///
    /// # Errors
    ///
    /// Returns [`FactoryError::ProtectedEnvironment`] when the environment
    /// names production or staging.
    pub fn new(database: &'a mut Database) -> Result<Self, FactoryError> {
        if let Some(environment) = protected_environment() {
            return Err(FactoryError::ProtectedEnvironment { environment });
        }
        Ok(Self::forced(database))
    }

    /// Bind a seeder **without** the environment check.
    ///
    /// For the rare deliberate case — a staging box that really is meant to
    /// hold generated data. Reaching for this in production is how test rows
    /// end up in front of customers.
    #[must_use]
    pub fn forced(database: &'a mut Database) -> Self {
        Self {
            database,
            faker: Faker::new(),
        }
    }

    /// Use a fixed seed, making every generated value reproducible.
    #[must_use]
    pub fn seeded(mut self, seed: u64) -> Self {
        self.faker = Faker::with_seed(seed);
        self
    }

    /// Generate values in a different locale.
    #[must_use]
    pub fn locale(mut self, locale: Locale) -> Self {
        self.faker.set_locale(locale);
        self
    }

    /// The faker backing this run, for one-off values between inserts.
    pub fn faker(&mut self) -> &mut Faker {
        &mut self.faker
    }

    /// Insert `count` rows of `M`.
    ///
    /// # Errors
    ///
    /// Returns [`FactoryError::Insert`] when the database rejects a row.
    pub async fn create<M: Factory>(&mut self, count: usize) -> Result<Vec<M>, FactoryError> {
        let mut created = Vec::with_capacity(count);
        for _ in 0..count {
            created.push(M::create_one(self.database, &mut self.faker).await?);
        }
        Ok(created)
    }

    /// Insert `count` rows of `M`, passing `argument` to each.
    ///
    /// # Errors
    ///
    /// Returns [`FactoryError::Insert`] when the database rejects a row.
    pub async fn create_with<M, A>(
        &mut self,
        count: usize,
        argument: A,
    ) -> Result<Vec<M>, FactoryError>
    where
        M: FactoryWith<A>,
        A: Clone,
    {
        let mut created = Vec::with_capacity(count);
        for _ in 0..count {
            created
                .push(M::create_one_with(self.database, &mut self.faker, argument.clone()).await?);
        }
        Ok(created)
    }
}

/// The protected environment name currently set, if any.
fn protected_environment() -> Option<String> {
    let values: Vec<Option<String>> = ENVIRONMENT_VARIABLES
        .iter()
        .map(|variable| std::env::var(variable).ok())
        .collect();
    classify_environment(&values)
}

/// Decide whether seeding is allowed, given the environment values in order.
///
/// Split out from the lookup so the policy is testable without mutating
/// process state — which the 2024 edition makes `unsafe`, and this workspace
/// forbids outright.
fn classify_environment(values: &[Option<String>]) -> Option<String> {
    for value in values {
        let Some(value) = value else {
            continue;
        };
        let normalized = value.trim().to_ascii_lowercase();
        if normalized.is_empty() {
            continue;
        }
        if PROTECTED_ENVIRONMENTS.contains(&normalized.as_str()) {
            return Some(normalized);
        }
        // The first variable that is set decides: a development value here
        // must not be vetoed by a stale one further down the list.
        return None;
    }
    None
}

/// Implement [`Factory`] (or [`FactoryWith`]) for a model.
///
/// The closure returns a Toasty create builder; the macro executes it. Writing
/// it this way means the builder's generated type never has to be named.
///
/// ```ignore
/// phoenix::factory! {
///     User, |f| User::create().name(f.name()).email(f.unique_email()),
/// }
/// phoenix::factory! {
///     Post, |f, user_id: i64| Post::create().title(f.sentence(6)).user_id(user_id),
/// }
/// ```
#[macro_export]
macro_rules! factory {
    ($model:ty, | $faker:ident | $build:expr $(,)?) => {
        impl $crate::factory::Factory for $model {
            fn create_one<'a>(
                database: &'a mut $crate::Database,
                faker: &'a mut $crate::factory::Faker,
            ) -> ::core::pin::Pin<
                ::std::boxed::Box<
                    dyn ::core::future::Future<
                            Output = ::core::result::Result<Self, $crate::factory::FactoryError>,
                        > + ::core::marker::Send
                        + 'a,
                >,
            > {
                ::std::boxed::Box::pin(async move {
                    let $faker = faker;
                    let builder = $build;
                    builder
                        .exec(database.toasty_mut())
                        .await
                        .map_err(|error| $crate::factory::FactoryError::Insert(error.to_string()))
                })
            }
        }
    };
    ($model:ty, | $faker:ident, $argument:ident : $argument_ty:ty | $build:expr $(,)?) => {
        impl $crate::factory::FactoryWith<$argument_ty> for $model {
            fn create_one_with<'a>(
                database: &'a mut $crate::Database,
                faker: &'a mut $crate::factory::Faker,
                $argument: $argument_ty,
            ) -> ::core::pin::Pin<
                ::std::boxed::Box<
                    dyn ::core::future::Future<
                            Output = ::core::result::Result<Self, $crate::factory::FactoryError>,
                        > + ::core::marker::Send
                        + 'a,
                >,
            > {
                ::std::boxed::Box::pin(async move {
                    let $faker = faker;
                    let builder = $build;
                    builder
                        .exec(database.toasty_mut())
                        .await
                        .map_err(|error| $crate::factory::FactoryError::Insert(error.to_string()))
                })
            }
        }
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build the value list `classify_environment` expects.
    fn environment(values: &[Option<&str>]) -> Vec<Option<String>> {
        values
            .iter()
            .map(|value| value.map(ToOwned::to_owned))
            .collect()
    }

    #[test]
    fn nothing_set_means_development() {
        assert_eq!(
            classify_environment(&environment(&[None, None, None])),
            None
        );
        assert_eq!(
            classify_environment(&environment(&[Some("  "), None, None])),
            None,
            "a blank value is not an answer"
        );
    }

    #[test]
    fn production_and_staging_are_refused_however_they_are_spelled() {
        for value in ["production", "PRODUCTION", " prod ", "Staging"] {
            assert_eq!(
                classify_environment(&environment(&[Some(value), None, None])).as_deref(),
                Some(value.trim().to_ascii_lowercase().as_str()),
                "`{value}` must be refused"
            );
        }
    }

    #[test]
    fn development_values_are_allowed() {
        for value in ["development", "dev", "test", "local", "ci"] {
            assert_eq!(
                classify_environment(&environment(&[Some(value), None, None])),
                None,
                "`{value}` may be seeded"
            );
        }
    }

    #[test]
    fn the_first_variable_that_is_set_decides() {
        // A stale APP_ENV must not veto an explicit PHOENIX_ENV...
        assert_eq!(
            classify_environment(&environment(&[Some("test"), Some("production"), None])),
            None
        );
        // ...but with PHOENIX_ENV unset, the next one is consulted.
        assert_eq!(
            classify_environment(&environment(&[None, Some("production"), None])).as_deref(),
            Some("production")
        );
        assert_eq!(
            classify_environment(&environment(&[None, None, Some("prod")])).as_deref(),
            Some("prod")
        );
    }

    #[test]
    fn the_refusal_says_what_to_do_about_it() {
        let error = FactoryError::ProtectedEnvironment {
            environment: "production".to_owned(),
        }
        .to_string();
        assert!(error.contains("development-only"), "{error}");
        assert!(error.contains("PHOENIX_ENV"), "{error}");
        assert!(error.contains("forced"), "{error}");
    }
}
