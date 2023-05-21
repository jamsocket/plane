use std::str::FromStr;

use quote::quote;
use darling::FromDeriveInput;
use syn::{self, parse_macro_input, parse::Parse, DeriveInput, Attribute, Meta, parse::ParseStream};
use proc_macro::TokenStream;

#[proc_macro]
pub fn make_answer(_item: TokenStream) -> TokenStream {
    "fn answer() -> u32 { 42 }".parse().unwrap()
}


#[proc_macro_derive(AnswerFn)]
pub fn derive_answer_fn(_item: TokenStream) -> TokenStream {
	"fn answer() -> u32 { 42 }".parse().unwrap()
}

struct NatsSubject {
	name: String,
	props: Vec<(String, String)>
}

impl Parse for NatsSubject {
	fn parse(input: ParseStream) -> Result<Self, syn::Error> {
		//assert to make sure thing is a big string
		let strput = input.to_string();
		let strput = strput.split('#');
		let mut out = strput.map(|each| { each.strip_suffix('.').unwrap().to_string() });
		let name = out.next_back().unwrap();
		let props = out.clone().zip(out.skip(1)).collect();

		Ok( Self {
			props,
			name
		})
	}
}

#[proc_macro_derive(TypedMessage, attributes(typed_message))]
pub fn typed_message_impl(input: TokenStream) -> TokenStream {
	//so I want this to look at the stuff in the message, then generate a subject
	//if cluster is specified, then cluster.{}
	//if backend specified then cluseter.{}.backend.{}.message

	//so finally I want
	/*
	#[derive(TypedMessage)]
	#[TypedMessage("cluster.#cluster.backend.#backend.stats")]
	pub struct BackendStatsMessage {
	pub cluster: ClusterName
	pub backend: BackendId

	}
	*/

	let input = parse_macro_input!(input as DeriveInput);
	let subj = parse_typed_message(&input.attrs).unwrap();

	let sub_str = subj.props.iter().map(|(car, _)| car.clone()).collect::<Vec<String>>().join("{}") + subj.name.as_str();
	let to_sub = subj.props.iter().map(|(_, cdr)| cdr.clone()).collect::<Vec<String>>().join(",");
	quote!{
		impl TypedMessage for #input.ident {
			type Response = NoResponse;

			fn subject(&self) -> String {
				format!(#sub_str, #to_sub)
			}
		}
	}.into()
}


fn parse_typed_message(attrs: &[Attribute]) -> Result<NatsSubject, syn::Error> {
	for attr in attrs {
		if attr.path().is_ident("typed_message") {
			let subj: NatsSubject = attr.parse_args()?;
			return Ok(subj);
		}
	}
	Err(syn::Error::new_spanned(attrs[0].clone(), "missing #[typed_message(\"...\")] attribute"))
}
