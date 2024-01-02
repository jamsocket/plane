extern crate proc_macro;
use proc_macro::TokenStream;
use quote::quote;
use syn::{ItemFn, Lit};

const DEFAULT_TEST_TIMEOUT: u64 = 60;

fn parse_timeout_attr(item: proc_macro2::TokenStream) -> u64 {
    if item.is_empty() {
        return DEFAULT_TEST_TIMEOUT;
    }
    let timeout: Result<Lit, _> = syn::parse2(item);
    match timeout {
        Ok(Lit::Int(int)) => int.base10_parse().unwrap(),
        _ => panic!("The #[plane_test] attribute should be a single integer."),
    }
}

fn plane_test_impl(item: proc_macro2::TokenStream, test_timeout: u64) -> proc_macro2::TokenStream {
    let func: ItemFn = syn::parse2(item).expect("#[plane_test] should annotate a function.");

    assert!(
        func.sig.asyncness.is_some(),
        "The function annotated by #[plane_test] should be async."
    );
    let mut sig = func.sig.clone();

    assert!(
        sig.inputs.len() == 1,
        "The function should have exactly one argument."
    );
    let first_arg = sig.inputs.first().unwrap();
    let arg_type = match first_arg {
        syn::FnArg::Typed(pat_type) => pat_type.ty.clone(),
        _ => panic!("The function should have exactly one argument."),
    };

    assert!(
        quote! { #arg_type }.to_string() == "TestEnvironment",
        "The argument of the function should be of type TestEnvironment, not {}.",
        quote! { #arg_type }
    );

    sig.asyncness = None;
    sig.inputs.clear();
    let block = func.block;
    let name = func.sig.ident.to_string();

    quote! {
        #[test]
        #sig {
            // If we don't include this, the developer doesn't have to import TestEnvironment,
            // which is confusing.
            let _: std::marker::PhantomData<#arg_type> = std::marker::PhantomData;

            common::run_test(#name, std::time::Duration::from_secs(#test_timeout), |mut env: common::test_env::TestEnvironment| {
                async move {
                    #block
                }
            })
        }
    }
}

/// This macro is used to annotate Plane integration tests.
/// Tests should take a single argument of type TestEnvironment.
/// They can use this test environment to create Plane resources.
/// The test environment will be cleaned up after the test is run,
/// unless the test fails.
#[proc_macro_attribute]
pub fn plane_test(attr: TokenStream, item: TokenStream) -> TokenStream {
    let timeout = parse_timeout_attr(attr.into());
    plane_test_impl(item.into(), timeout).into()
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn test_parse_no_attributes() {
        let input = quote! {};
        let expected = DEFAULT_TEST_TIMEOUT;

        let actual = parse_timeout_attr(input);
        assert_eq!(expected, actual);
    }

    #[test]
    fn test_parse_with_timeout() {
        let input = quote! {3};
        let expected = 3;

        let actual = parse_timeout_attr(input);
        assert_eq!(expected, actual);
    }

    #[test]
    fn test_plane_test_macro() {
        let input = quote! {
            async fn my_test(env: TestEnvironment) {
                test_body();
            }
        };

        let expected = quote! {
            #[test]
            fn my_test() {
                let _: std::marker::PhantomData<TestEnvironment> = std::marker::PhantomData;

                common::run_test("my_test", std::time::Duration::from_secs(10u64), |mut env: common::test_env::TestEnvironment| {
                    async move {
                        {
                            test_body();
                        }
                    }
                })
            }
        };

        let actual = plane_test_impl(input, 10);
        assert_eq!(expected.to_string(), actual.to_string());
    }
}
