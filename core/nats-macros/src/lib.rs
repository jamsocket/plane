use quote::{quote};
use syn::{self, parse_macro_input, parse::Parse, DeriveInput, Attribute, parse::ParseStream, LitStr, __private::TokenStream2};
use proc_macro::TokenStream;

#[proc_macro]
pub fn make_answer(_item: TokenStream) -> TokenStream {
    "fn answer() -> u32 { 42 }".parse().unwrap()
}


#[proc_macro_derive(AnswerFn)]
pub fn derive_answer_fn(_item: TokenStream) -> TokenStream {
	"fn answer() -> u32 { 42 }".parse().unwrap()
}

#[derive(Debug)]
struct NatsSubject {
	name: String,
	props: Vec<(String, String)>
}

impl Parse for NatsSubject {
	fn parse(input: ParseStream) -> Result<Self, syn::Error> {
		let lit: LitStr = input.parse()?;
		let strput = lit.value(); //borrow checker wrangling
		let mut out = strput.split('.').map(|each| { each.trim_start_matches("#").to_string() });
		let name = out.next_back().unwrap();
		let props = out.clone().collect::<Vec<String>>().chunks_exact(2).map(|chunk| (chunk[0].clone(), chunk[1].clone())).collect();

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

	let ast = parse_macro_input!(input as DeriveInput);
	match parse_typed_message(&ast.attrs) {
		Ok(subj) => {
			let sub_str = subj.props.iter().map(
				|(car, _)| car.clone()).collect::<Vec<String>>().join(".{}.") + ".{}." + subj.name.as_str();
			let to_sub : TokenStream2 = syn::parse_str(subj.props.iter().map(|(_, cdr)| "self.".to_owned() + cdr.clone().as_str() + ".to_string()").collect::<Vec<String>>().join(",").as_str()).unwrap();
			let typ = ast.ident;
			quote!{
				impl TypedMessage for #typ {
					type Response = NoReply;

					fn subject(&self) -> String {
						format!(#sub_str, #to_sub)
					}
				}
			}.into()
		}
		Err(e) => {
	            e.to_compile_error().into()		
		}
	}
}


fn parse_typed_message(attrs: &[Attribute]) -> Result<NatsSubject, syn::Error> {
	for attr in attrs {
		if attr.path().is_ident("typed_message") {
			let subj = attr.parse_args::<NatsSubject>();
			match subj {
				Ok(subj) => {
					return Ok(subj);
				}
				Err(e) => return Err(syn::Error::new_spanned(attr, e)),
			}
		}
	}
	Err(syn::Error::new_spanned(attrs[0].clone(), "missing #[typed_message(\"...\")] attribute"))
}
