use proc_macro::TokenStream;

/// Mark a Rust DTO as a Phoenix input, resource, page, or shared contract.
///
/// The attribute is intentionally representation-neutral. The Vite contract
/// exporter reads the Rust declaration and applies Serde's wire rules.
#[proc_macro_attribute]
pub fn contract(_metadata: TokenStream, item: TokenStream) -> TokenStream {
    item
}
