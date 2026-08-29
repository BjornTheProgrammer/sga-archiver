extern crate proc_macro;

use proc_macro::TokenStream;
use quote::{format_ident, quote};
use syn::{parse::{Parse, ParseStream}, parse_macro_input, Expr, Ident, Token, Type};

struct ReadFieldInput {
    reader: Expr,
    _comma1: Token![,],
    enum_name: Ident,
    _colon2: Token![::],
    variant: Ident,
    _comma2: Token![,],
    output_type: Type,
}

impl Parse for ReadFieldInput {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        Ok(ReadFieldInput {
            reader: input.parse()?,
            _comma1: input.parse()?,
            enum_name: input.parse()?,
            _colon2: input.parse()?,
            variant: input.parse()?,
            _comma2: input.parse()?,
            output_type: input.parse()?,
        })
    }
}

/// This is a macro which automates creating a custom error if there is a parse error, and reads the specified uint and int type from the buffer, only as a Little Endian.
#[proc_macro]
pub fn read_field(input: TokenStream) -> TokenStream {
    let ReadFieldInput {
        reader,
        enum_name,
        variant,
        output_type,
        ..
    } = parse_macro_input!(input as ReadFieldInput);

    let output_type = quote!(#output_type).to_string();

    let method = format_ident!("read_{}", output_type);

    let generated = if output_type == "u8" {
        quote! {{
            use byteorder::{LittleEndian, BigEndian, ReadBytesExt};

            #reader.#method().map_err(|_| {
                #enum_name::#variant("Failed to parse version number".to_string())
            })
        }}
    } else {
        quote! {{
            use byteorder::{LittleEndian, BigEndian, ReadBytesExt};

            #reader.#method::<LittleEndian>().map_err(|_| {
                #enum_name::#variant("Failed to parse version number".to_string())
            })
        }}
    };

    generated.into()
}
