//! `#[phoenix::model]`: convention-first models on top of the Toasty derive.
//!
//! Toasty can express every relation shape, but it asks you to spell all of it
//! out — the foreign-key field, the `key = …` it maps to, and the
//! `references = …` on the far side. That is exactly right for a general ORM
//! and exactly too much for the 95% case, which is always "this row belongs to
//! that row, joined on `<name>_id`".
//!
//! This attribute fills in that 95% and then gets out of the way: every
//! convention below is overridden by writing the thing out, and the expansion
//! is plain Toasty, so nothing here is a dead end.

use proc_macro2::{Span, TokenStream};
use quote::{ToTokens, quote};
use syn::{
    Attribute, Field, Fields, Ident, ItemStruct, Meta, Token, Type, Visibility,
    punctuated::Punctuated, spanned::Spanned,
};

/// Rewrite a model struct with Phoenix's conventions applied.
pub fn expand(item: ItemStruct) -> syn::Result<TokenStream> {
    let mut item = item;
    let Fields::Named(_) = &item.fields else {
        return Err(syn::Error::new(
            item.fields.span(),
            "#[phoenix::model] requires a struct with named fields",
        ));
    };

    ensure_table_attribute(&mut item);
    let relations = collect_relations(&item)?;
    rewrite_relation_attributes(&mut item, &relations);
    add_foreign_key_fields(&mut item, &relations);
    ensure_primary_key(&mut item);
    let derives = missing_derives(&item.attrs);

    Ok(quote! {
        #derives
        #item
    })
}

/// A `#[belongs_to]` field and the conventions resolved for it.
struct Relation {
    /// The relation field's own name (`user`).
    field: Ident,
    /// Foreign-key field on this model (`user_id`).
    key: Ident,
    /// Field referenced on the target model (`id`).
    references: Ident,
    /// Whether the relation — and therefore the foreign key — is nullable.
    nullable: bool,
    /// Index of the attribute to rewrite.
    attribute: usize,
    /// Index of the field itself.
    position: usize,
}

/// Resolve conventions for every `#[belongs_to]` field.
fn collect_relations(item: &ItemStruct) -> syn::Result<Vec<Relation>> {
    let mut relations = Vec::new();
    for (position, field) in item.fields.iter().enumerate() {
        let Some(attribute) = field
            .attrs
            .iter()
            .position(|attr| attr.path().is_ident("belongs_to"))
        else {
            continue;
        };
        let Some(name) = field.ident.clone() else {
            continue;
        };
        let (key, references) = relation_arguments(&field.attrs[attribute], &name)?;
        relations.push(Relation {
            field: name,
            key,
            references,
            nullable: is_nullable_relation(&field.ty),
            attribute,
            position,
        });
    }
    Ok(relations)
}

/// Read `key = …` / `references = …` from a `#[belongs_to]`, defaulting to the
/// conventions: `<field>_id` referencing `id`.
fn relation_arguments(attribute: &Attribute, field: &Ident) -> syn::Result<(Ident, Ident)> {
    let mut key = None;
    let mut references = None;

    if !matches!(attribute.meta, Meta::Path(_)) {
        let nested = attribute.parse_args_with(Punctuated::<Meta, Token![,]>::parse_terminated)?;
        for meta in nested {
            let Meta::NameValue(pair) = meta else {
                return Err(syn::Error::new(
                    meta.span(),
                    "#[belongs_to] accepts `key = field` and `references = field`",
                ));
            };
            let target = if pair.path.is_ident("key") {
                &mut key
            } else if pair.path.is_ident("references") {
                &mut references
            } else {
                return Err(syn::Error::new(
                    pair.path.span(),
                    "#[belongs_to] accepts `key = field` and `references = field`",
                ));
            };
            *target = Some(expression_ident(&pair.value)?);
        }
    }

    Ok((
        key.unwrap_or_else(|| foreign_key_ident(field)),
        references.unwrap_or_else(|| Ident::new("id", field.span())),
    ))
}

/// `user` → `user_id`.
fn foreign_key_ident(field: &Ident) -> Ident {
    Ident::new(&format!("{field}_id"), field.span())
}

/// Accept a bare identifier on the right of `key = …`.
fn expression_ident(value: &syn::Expr) -> syn::Result<Ident> {
    if let syn::Expr::Path(path) = value
        && let Some(ident) = path.path.get_ident()
    {
        return Ok(ident.clone());
    }
    Err(syn::Error::new(
        value.span(),
        "expected a field name, for example `key = author_id`",
    ))
}

/// Whether the relation resolves to `Option<T>` — including through
/// `Deferred<Option<T>>` — which makes its foreign key nullable too.
fn is_nullable_relation(ty: &Type) -> bool {
    match outer_generic(ty) {
        Some((name, inner)) if name == "Option" => {
            let _ = inner;
            true
        }
        Some((_, inner)) => is_nullable_relation(inner),
        None => false,
    }
}

/// Split `Wrapper<Inner>` into its name and first type argument.
fn outer_generic(ty: &Type) -> Option<(String, &Type)> {
    let Type::Path(path) = ty else {
        return None;
    };
    let segment = path.path.segments.last()?;
    let syn::PathArguments::AngleBracketed(arguments) = &segment.arguments else {
        return None;
    };
    let syn::GenericArgument::Type(inner) = arguments.args.first()? else {
        return None;
    };
    Some((segment.ident.to_string(), inner))
}

/// Replace each shorthand `#[belongs_to]` with the full Toasty form.
fn rewrite_relation_attributes(item: &mut ItemStruct, relations: &[Relation]) {
    for relation in relations {
        let Some(field) = item.fields.iter_mut().nth(relation.position) else {
            continue;
        };
        let key = &relation.key;
        let references = &relation.references;
        field.attrs[relation.attribute] = syn::parse_quote! {
            #[belongs_to(key = #key, references = #references)]
        };
    }
}

/// Declare any foreign-key field a relation needs and the author did not write.
///
/// The convention key type is `i64`, matching the primary key this attribute
/// generates. A model keyed by anything else declares its own foreign key —
/// guessing the far side's key type from here is not possible, and guessing
/// wrong would surface as an unrelated type error inside the Toasty derive.
fn add_foreign_key_fields(item: &mut ItemStruct, relations: &[Relation]) {
    let Fields::Named(fields) = &mut item.fields else {
        return;
    };
    for relation in relations {
        let key = &relation.key;
        if fields
            .named
            .iter()
            .any(|field| field.ident.as_ref().is_some_and(|name| name == key))
        {
            continue;
        }
        let visibility = relation_visibility(&fields.named, relation);
        let field: Field = if relation.nullable {
            syn::parse_quote! { #visibility #key: ::core::option::Option<i64> }
        } else {
            syn::parse_quote! { #visibility #key: i64 }
        };
        // Insert next to its relation so the generated struct still reads in
        // the order the author wrote it.
        fields.named.insert(relation.position, field);
    }
}

/// A synthesized foreign key inherits the visibility of its relation field, so
/// it is reachable exactly where the relation is.
fn relation_visibility(fields: &Punctuated<Field, Token![,]>, relation: &Relation) -> Visibility {
    fields
        .iter()
        .find(|field| {
            field
                .ident
                .as_ref()
                .is_some_and(|name| *name == relation.field)
        })
        .map_or(Visibility::Inherited, |field| field.vis.clone())
}

/// Give the model an auto-increment `id` unless it already declares a key.
fn ensure_primary_key(item: &mut ItemStruct) {
    let Fields::Named(fields) = &mut item.fields else {
        return;
    };
    let has_key = fields
        .named
        .iter()
        .any(|field| field.attrs.iter().any(|attr| attr.path().is_ident("key")))
        || item.attrs.iter().any(|attr| attr.path().is_ident("key"));
    if has_key {
        return;
    }
    let field: Field = syn::parse_quote! {
        #[key]
        #[auto]
        pub id: i64
    };
    fields.named.insert(0, field);
}

/// Give the model a table name unless it already declares one.
fn ensure_table_attribute(item: &mut ItemStruct) {
    if item.attrs.iter().any(|attr| attr.path().is_ident("table")) {
        return;
    }
    let table = table_name(&item.ident.to_string());
    item.attrs.push(syn::parse_quote! { #[table = #table] });
}

/// Derives every model needs, minus the ones already written.
fn missing_derives(attrs: &[Attribute]) -> TokenStream {
    let existing = derived_names(attrs);
    let mut wanted: Vec<TokenStream> = Vec::new();
    if !existing.iter().any(|name| name == "Debug") {
        wanted.push(quote!(::core::fmt::Debug));
    }
    if !existing
        .iter()
        .any(|name| name == "Model" || name.ends_with("::Model"))
    {
        wanted.push(quote!(::phoenix::database::Model));
    }
    if wanted.is_empty() {
        TokenStream::new()
    } else {
        quote! { #[derive(#(#wanted),*)] }
    }
}

/// Names inside every `#[derive(...)]` on the item.
fn derived_names(attrs: &[Attribute]) -> Vec<String> {
    let mut names = Vec::new();
    for attribute in attrs {
        if !attribute.path().is_ident("derive") {
            continue;
        }
        let parser = Punctuated::<syn::Path, Token![,]>::parse_terminated;
        let Ok(paths) = attribute.parse_args_with(parser) else {
            continue;
        };
        for path in paths {
            names.push(path.to_token_stream().to_string().replace(' ', ""));
        }
    }
    names
}

/// `BlogPost` → `blog_posts`.
pub fn table_name(type_name: &str) -> String {
    pluralize(&snake_case(type_name))
}

/// `BlogPost` → `blog_post`.
fn snake_case(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 4);
    for (index, character) in value.char_indices() {
        if character.is_uppercase() {
            if index != 0 {
                out.push('_');
            }
            out.extend(character.to_lowercase());
        } else {
            out.push(character);
        }
    }
    out
}

/// English pluralization covering the shapes table names actually take.
///
/// Deliberately not a full inflector: an irregular noun is one `#[table = …]`
/// away, and a wrong guess is visible the first time the migration runs.
fn pluralize(value: &str) -> String {
    const SIBILANT_ENDINGS: [&str; 5] = ["s", "x", "z", "ch", "sh"];

    if value.is_empty() {
        return value.to_owned();
    }
    if let Some(stem) = value.strip_suffix('y') {
        let vowel_before = stem
            .chars()
            .last()
            .is_some_and(|character| "aeiou".contains(character));
        if !vowel_before {
            return format!("{stem}ies");
        }
    }
    if SIBILANT_ENDINGS
        .iter()
        .any(|ending| value.ends_with(ending))
    {
        return format!("{value}es");
    }
    format!("{value}s")
}

/// Span helper for error reporting from the entry point.
pub fn call_site() -> Span {
    Span::call_site()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_names_follow_the_convention() {
        assert_eq!(table_name("User"), "users");
        assert_eq!(table_name("Post"), "posts");
        assert_eq!(table_name("BlogPost"), "blog_posts");
        assert_eq!(table_name("Category"), "categories");
        assert_eq!(table_name("Day"), "days", "a vowel before -y keeps the y");
        assert_eq!(table_name("Address"), "addresses");
        assert_eq!(table_name("Box"), "boxes");
        assert_eq!(table_name("Branch"), "branches");
        assert_eq!(table_name("Dish"), "dishes");
    }

    #[test]
    fn foreign_keys_follow_the_relation_field_name() {
        let field = Ident::new("user", Span::call_site());
        assert_eq!(foreign_key_ident(&field).to_string(), "user_id");
        let field = Ident::new("published_by", Span::call_site());
        assert_eq!(foreign_key_ident(&field).to_string(), "published_by_id");
    }

    #[test]
    fn nullability_is_read_through_the_deferred_wrapper() {
        let required: Type = syn::parse_quote!(Deferred<User>);
        let optional: Type = syn::parse_quote!(Deferred<Option<User>>);
        let bare: Type = syn::parse_quote!(User);
        let bare_optional: Type = syn::parse_quote!(Option<User>);
        assert!(!is_nullable_relation(&required));
        assert!(is_nullable_relation(&optional));
        assert!(!is_nullable_relation(&bare));
        assert!(is_nullable_relation(&bare_optional));
    }

    /// The whole point: what the author writes versus what Toasty receives.
    #[test]
    fn the_shorthand_expands_to_full_toasty() {
        let item: ItemStruct = syn::parse_quote! {
            pub struct Post {
                pub title: String,
                #[belongs_to]
                pub user: Deferred<User>,
            }
        };
        let expanded = expand(item).expect("expands").to_string();

        assert!(expanded.contains("table = \"posts\""), "{expanded}");
        assert!(
            expanded.contains("belongs_to (key = user_id , references = id)"),
            "{expanded}"
        );
        assert!(expanded.contains("pub user_id : i64"), "{expanded}");
        assert!(expanded.contains("# [key]"), "{expanded}");
        assert!(expanded.contains("# [auto]"), "{expanded}");
        assert!(expanded.contains("pub id : i64"), "{expanded}");
        assert!(
            expanded.contains(":: phoenix :: database :: Model"),
            "{expanded}"
        );
    }

    #[test]
    fn every_convention_yields_to_an_explicit_declaration() {
        let item: ItemStruct = syn::parse_quote! {
            #[derive(Clone, Debug)]
            #[table = "articles"]
            pub struct Post {
                #[key]
                pub slug: String,
                pub author_id: i64,
                #[belongs_to(key = author_id, references = uuid)]
                pub author: Deferred<User>,
            }
        };
        let expanded = expand(item).expect("expands").to_string();

        assert!(expanded.contains("table = \"articles\""), "{expanded}");
        assert!(
            expanded.contains("belongs_to (key = author_id , references = uuid)"),
            "{expanded}"
        );
        assert!(
            !expanded.contains("pub id : i64"),
            "a declared key is not replaced: {expanded}"
        );
        assert_eq!(
            expanded.matches("author_id :").count(),
            1,
            "a declared foreign key is not duplicated: {expanded}"
        );
        assert!(
            !expanded.contains("core :: fmt :: Debug"),
            "an existing Debug derive is not repeated: {expanded}"
        );
    }

    #[test]
    fn a_nullable_relation_gets_a_nullable_foreign_key() {
        let item: ItemStruct = syn::parse_quote! {
            pub struct Post {
                #[belongs_to]
                pub editor: Deferred<Option<User>>,
            }
        };
        let expanded = expand(item).expect("expands").to_string();
        assert!(
            expanded.contains("pub editor_id : :: core :: option :: Option < i64 >"),
            "{expanded}"
        );
    }

    #[test]
    fn a_partial_override_keeps_the_other_convention() {
        let item: ItemStruct = syn::parse_quote! {
            pub struct Post {
                #[belongs_to(key = author_id)]
                pub author: Deferred<User>,
            }
        };
        let expanded = expand(item).expect("expands").to_string();
        assert!(
            expanded.contains("belongs_to (key = author_id , references = id)"),
            "references still defaults to id: {expanded}"
        );
        assert!(expanded.contains("pub author_id : i64"), "{expanded}");
    }

    #[test]
    fn unknown_relation_arguments_are_rejected() {
        let item: ItemStruct = syn::parse_quote! {
            pub struct Post {
                #[belongs_to(on_delete = cascade)]
                pub user: Deferred<User>,
            }
        };
        let error = expand(item).expect_err("unknown argument");
        assert!(
            error.to_string().contains("references"),
            "the error names what is accepted: {error}"
        );
    }

    #[test]
    fn has_many_and_has_one_are_left_alone() {
        // Toasty already infers the pair for these; there is no convention to
        // add, so the attribute must survive untouched.
        let item: ItemStruct = syn::parse_quote! {
            pub struct User {
                #[has_many]
                pub posts: Deferred<Vec<Post>>,
                #[has_one]
                pub profile: Deferred<Option<Profile>>,
            }
        };
        let expanded = expand(item).expect("expands").to_string();
        assert!(expanded.contains("# [has_many]"), "{expanded}");
        assert!(expanded.contains("# [has_one]"), "{expanded}");
        assert!(!expanded.contains("posts_id"), "{expanded}");
        assert!(!expanded.contains("profile_id"), "{expanded}");
    }
}
