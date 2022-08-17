extern crate proc_macro;
use proc_macro::TokenStream;
use quote::quote;
use syn::ItemFn;

fn integration_test_impl(item: proc_macro2::TokenStream) -> proc_macro2::TokenStream {
    let func: ItemFn = syn::parse2(item).expect("#[integration_test] should annotate a function.");
    assert!(func.sig.asyncness.is_some(), "The function annotated by #[integration_test] should be async.");
    let mut sig = func.sig.clone();
    sig.asyncness = None;
    let block = func.block;
    let name = func.sig.ident.to_string();

    quote! {
        #[test]
        #sig {
            let context = dev::TestContext::new(#name);
            dev::TEST_CONTEXT.with(|cell| cell.replace(Some(context)));
            let scratch_dir = dev::scratch_dir("logs");

            let file_appender = tracing_appender::rolling::RollingFileAppender::new(
                tracing_appender::rolling::Rotation::NEVER, scratch_dir, "test-log.txt");
            
            let (non_blocking, _guard) = tracing_appender::non_blocking(file_appender);

            let subscriber = tracing_subscriber::fmt()
                .compact()
                .with_ansi(false)
                .with_writer(non_blocking)
                .finish();
            
            let dispatcher = tracing::dispatcher::Dispatch::new(subscriber);
            let _guard = tracing::dispatcher::set_default(&dispatcher);

            let result = tokio::runtime::Runtime::new().unwrap().block_on(async move {
                #block
            });

            if result.is_ok() {
                dev::TEST_CONTEXT.with(|cell|
                    tokio::runtime::Runtime::new().unwrap().block_on(async {
                        cell.borrow().as_ref().unwrap().teardown().await;
                    })
                );    
            }

            result
        }
    }
}

#[proc_macro_attribute]
pub fn integration_test(_: TokenStream, item: TokenStream) -> TokenStream {
    integration_test_impl(item.into()).into()
}
