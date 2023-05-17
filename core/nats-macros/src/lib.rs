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

#[derive(FromDeriveInput)]
#[darling(attributes(typed_message))]
struct Opts {
	name: String,
	response: String 
}

#[proc_macro_derive(TypedMessage, attributes(typed_message))]
pub fn typed_message_impl(input: TokenStream) -> TokenStream {
	//so I want this to look at the stuff in the message, then generate a subject
	//if cluster is specified, then cluster.{}
	//if backend specified then cluseter.{}.backend.{}.message

	//so finally I want
	/*
	#[derive(TypedMessage)]
	#[TypedMessage(name=stats)]
	pub struct BackendStatsMessage {
	#[cluster]
	pub cluster: ClusterName
	#[backend]
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
