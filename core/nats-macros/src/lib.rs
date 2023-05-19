use std::str::FromStr;

use quote::quote;
use darling::FromDeriveInput;
use syn::{self, parse_macro_input, DeriveInput};
use proc_macro::TokenStream;

#[proc_macro]
pub fn make_answer(_item: TokenStream) -> TokenStream {
    "fn answer() -> u32 { 42 }".parse().unwrap()
}


#[proc_macro_derive(AnswerFn)]
pub fn derive_answer_fn(_item: TokenStream) -> TokenStream {
	"fn answer() -> u32 { 42 }".parse().unwrap()
}

struct NatsSubjectString(String);

impl FromStr for NatsSubjectString {
	type Err = String;
	fn from_str(inp: &str) -> Result<Self, Self::Err> {


		Ok("test".into())
	}

}

#[derive(FromDeriveInput)]
#[darling(attributes(typed_message))]
struct Opts {
	subject: NatsSubjectString
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
	let opts = Opts::from_derive_input(&input).expect("Wrong options");
	let response = opts.response;
	let subj = format!("{}", opts.name);
	let gen = quote!{
		impl TypedMessage for #input.ident {
			type Response = #response;

			fn subject(&self) -> String {
				format!(#subj, self.backend_id.id())
			}
		}
	};

	gen.into()
}
