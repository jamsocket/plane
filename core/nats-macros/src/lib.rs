use darling::{FromDeriveInput, FromMeta};
use proc_macro::TokenStream;
use quote::quote;
use syn::{self, parse_macro_input, DeriveInput, token, Token};

#[derive(FromDeriveInput)]
#[darling(attributes(typed_message))]
struct Opts {
    subject: NatsSubjectMacroInvocation,
    #[darling(default)]
    response: ResponseType,
}

#[derive(FromMeta, Clone, Debug)]
struct ResponseType(syn::Type);

impl Default for ResponseType {
    fn default() -> Self {
        ResponseType(syn::Type::from_string("NoReply").unwrap())
    }
}

#[derive(Debug)]
struct NatsSubjectMacroInvocation(syn::Macro);

impl FromMeta for NatsSubjectMacroInvocation {
    fn from_string(value: &str) -> darling::Result<Self> {
        let props: Vec<&str> = value
            .split('.')
            .filter_map(|ea| ea.strip_prefix('#'))
            .collect();
        let mut fstring = value.to_string();
        for prop in props.clone() {
            fstring = fstring.replace(&("#".to_owned() + prop), "{}");
        }
		let as_subject_component_path : syn::Path = match std::env::var("CARGO_PKG_NAME").unwrap().as_str() {
			"plane-core" => syn::parse_quote!{ crate::types::AsSubjectComponent },
			_ => syn::parse_quote!{ plane_core::types::AsSubjectComponent }
		};

        let propexpr = props
            .iter()
            .map(|prop| {
				syn::parse_quote!{ #as_subject_component_path::as_subject_component( &self.#prop ) }
				/*
                "crate::types::AsSubjectComponent::as_subject_component(&self.".to_owned()
                    + prop
                    + ")"
				*/
            })
            .reduce(|a: syn::Expr, b| syn::parse_quote!{ #a , #b });

        let sub: syn::Macro = syn::parse_quote!{
			format!(#fstring, #propexpr)
		};
		/*
            syn::parse_str(&("format!(\"".to_owned() + fstring.as_str() + "\"," + &propstr + ")"))
                .unwrap();
		*/

        Ok(NatsSubjectMacroInvocation(sub))
    }
}

#[proc_macro_derive(TypedMessage, attributes(typed_message))]
pub fn typed_message_impl(input: TokenStream) -> TokenStream {
    let ast = parse_macro_input!(input as DeriveInput);
	let typed_message : syn::Path = match std::env::var("CARGO_PKG_NAME").unwrap().as_str() {
		"plane-core" => syn::parse2( quote!{ crate::nats::TypedMessage }).unwrap(),
		_ => syn::parse2( quote!{ plane_core::nats::TypedMessage }).unwrap()
	};
    let opts = Opts::from_derive_input(&ast).expect("Wrong options!");
    let typ = ast.ident;
    let resp_typ: syn::Type = opts.response.0;
    let fmacro = opts.subject.0;
    quote! {
        impl #typed_message for #typ {
            type Response = #resp_typ;

            fn subject(&self) -> String {
                #fmacro
            }
        }
    }
    .into()
}
