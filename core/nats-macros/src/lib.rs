use quote::{quote};
use syn::{self, parse_macro_input, parse::Parse, DeriveInput, Attribute, parse::ParseStream, LitStr, __private::TokenStream2};
use proc_macro::TokenStream;

#[derive(Debug)]
struct NatsSubject {
	fstring: String,
	props: Vec<String>
}

impl Parse for NatsSubject {
	fn parse(input: ParseStream) -> Result<Self, syn::Error> {
		let lit: LitStr = input.parse()?;
		let props: Vec<String> = lit.value()
			.split('.').map(|ea| ea.strip_prefix('#')).filter(|ea| ea.is_some())
			.map(|ea| ea.unwrap().to_string()).collect();
		let mut fstring = lit.value().clone();
		for prop in props.clone() {
			fstring = fstring.replace(&("#".to_owned()+prop.as_str()), "{}");
		}

		Ok( Self {
			props,
			fstring
		})
	}
}

#[proc_macro_derive(TypedMessage, attributes(typed_message))]
pub fn typed_message_impl(input: TokenStream) -> TokenStream {
	let ast = parse_macro_input!(input as DeriveInput);
	match parse_typed_message(&ast.attrs) {
		Ok(subj) => {
			let to_sub : TokenStream2 = syn::parse_str(
				subj.props.iter().map(|cdr| "self.".to_owned() + cdr.clone().as_str() + ".to_string()").collect::<Vec<String>>().join(",").as_str()).unwrap();
			let typ = ast.ident;
			let fstr = subj.fstring;
			quote!{
				impl TypedMessage for #typ {
					type Response = NoReply;

					fn subject(&self) -> String {
						format!(#fstr, #to_sub)
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
