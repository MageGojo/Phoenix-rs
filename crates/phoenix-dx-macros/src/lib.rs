use std::{fs, path::PathBuf};

use proc_macro::TokenStream;
use quote::{format_ident, quote};
use syn::{LitStr, parse_macro_input};

/// Discover sorted `*.rs` files below the application's route directory and
/// merge the `pub fn routes() -> Routes` value exported by each file.
#[proc_macro]
pub fn mount_routes(input: TokenStream) -> TokenStream {
    let relative = if input.is_empty() {
        "routes".to_owned()
    } else {
        parse_macro_input!(input as LitStr).value()
    };
    expand_routes(&relative)
        .unwrap_or_else(|message| compile_error(&message))
        .into()
}

fn expand_routes(relative: &str) -> Result<proc_macro2::TokenStream, String> {
    let root = PathBuf::from(
        std::env::var("CARGO_MANIFEST_DIR")
            .map_err(|_| "Phoenix could not read CARGO_MANIFEST_DIR".to_owned())?,
    )
    .join(relative);
    let entries = fs::read_dir(&root).map_err(|error| {
        format!(
            "Phoenix route directory {} cannot be read: {error}",
            root.display()
        )
    })?;
    let mut files = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "rs"))
        .filter(|path| path.file_name().is_some_and(|name| name != "mod.rs"))
        .collect::<Vec<_>>();
    files.sort();
    if files.is_empty() {
        return Err(format!(
            "Phoenix route directory {} contains no route files",
            root.display()
        ));
    }

    let modules = files.iter().enumerate().map(|(index, path)| {
        let module = format_ident!("__phoenix_route_file_{index}");
        let path = LitStr::new(&path.to_string_lossy(), proc_macro2::Span::call_site());
        quote! {
            #[path = #path]
            mod #module;
        }
    });
    let merges = files.iter().enumerate().map(|(index, _)| {
        let module = format_ident!("__phoenix_route_file_{index}");
        quote! {
            routes = routes.merge(#module::routes());
        }
    });
    Ok(quote! {{
        #(#modules)*
        let mut routes = ::phoenix::routing::Routes::new();
        #(#merges)*
        routes
    }})
}

fn compile_error(message: &str) -> proc_macro2::TokenStream {
    quote! { compile_error!(#message) }
}
