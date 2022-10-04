extern crate proc_macro;
use proc_macro::TokenStream;
use quote::quote;
use syn::ItemFn;

fn integration_test_impl(item: proc_macro2::TokenStream) -> proc_macro2::TokenStream {
    let func: ItemFn = syn::parse2(item).expect("#[integration_test] should annotate a function.");
    assert!(
        func.sig.asyncness.is_some(),
        "The function annotated by #[integration_test] should be async."
    );
    let mut sig = func.sig.clone();
    sig.asyncness = None;
    let block = func.block;
    let name = func.sig.ident.to_string();

    quote! {
        #[test]
        #sig {
            plane_dev::run_test(#name, async move {
                #block
            })
        }
    }
}

#[proc_macro_attribute]
pub fn integration_test(_: TokenStream, item: TokenStream) -> TokenStream {
    integration_test_impl(item.into()).into()
}
