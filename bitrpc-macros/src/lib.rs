use heck::ToUpperCamelCase;
use proc_macro::TokenStream;
use quote::{format_ident, quote};
use syn::parse::{Parse, Parser};
use syn::punctuated::Punctuated;
use syn::token::Comma;
use syn::{
    parse_macro_input,
    parse_quote,
    spanned::Spanned,
    Expr,
    FnArg,
    Ident,
    ItemTrait,
    LitStr,
    Pat,
    Path,
    PathArguments,
    TraitItem,
    Type,
    TypeParamBound,
};

#[proc_macro_attribute]
pub fn service(attr: TokenStream, item: TokenStream) -> TokenStream {
    let parser = Punctuated::<KeyValue, Comma>::parse_terminated;
    let args_tokens = proc_macro2::TokenStream::from(attr);
    let args = match parser.parse2(args_tokens) {
        Ok(value) => value,
        Err(err) => return err.into_compile_error().into(),
    };

    let mut input = parse_macro_input!(item as ItemTrait);

    match expand_service(args, &mut input) {
        Ok(tokens) => tokens.into(),
        Err(err) => err.to_compile_error().into(),
    }
}

struct KeyValue {
    key: Ident,
    value: Expr,
}

impl Parse for KeyValue {
    fn parse(input: syn::parse::ParseStream) -> syn::Result<Self> {
        let key: Ident = input.parse()?;
        input.parse::<syn::Token![=]>()?;
        let value: Expr = input.parse()?;
        Ok(Self { key, value })
    }
}

struct ServiceOptions {
    request_ident: Ident,
    response_ident: Ident,
    client_ident: Ident,
    error_path: Path,
}

struct MethodArg {
    ident: Ident,
    ty: Type,
}

struct MethodInfo {
    method_ident: Ident,
    request_struct_ident: Ident,
    request_fields: Vec<MethodArg>,
    method_inputs: Vec<syn::PatType>,
    success_ty: Type,
    name_literal: LitStr,
}

fn expand_service(
    args: Punctuated<KeyValue, Comma>,
    input: &mut ItemTrait,
) -> syn::Result<proc_macro2::TokenStream> {
    ensure_async_trait(input)?;

    let options = parse_service_options(args, &input.ident)?;

    let methods = collect_methods(input)?;

    if methods.is_empty() {
        return Err(syn::Error::new(
            input.ident.span(),
            "RPC traits must declare at least one method",
        ));
    }

    let trait_ident = &input.ident;
    let vis = &input.vis;
    let request_ident = &options.request_ident;
    let response_ident = &options.response_ident;
    let client_ident = &options.client_ident;
    let error_path = &options.error_path;

    let mut request_structs = Vec::new();
    let mut request_variants = Vec::new();
    let mut response_variants = Vec::new();
    let mut request_variant_names = Vec::new();
    let mut response_variant_names = Vec::new();
    let mut dispatch_arms = Vec::new();
    let mut client_methods = Vec::new();

    // Generate 256 placeholder variants for stable encoding
    const MAX_METHODS: usize = 256;
    
    if methods.len() > MAX_METHODS {
        return Err(syn::Error::new(
            input.ident.span(),
            format!("RPC traits cannot have more than {} methods", MAX_METHODS),
        ));
    }
    
    // Map methods to placeholder indices based on trait definition order
    for (method_idx, method_info) in methods.iter().enumerate() {
        let MethodInfo {
            method_ident,
            request_struct_ident,
            request_fields,
            method_inputs,
            success_ty,
            name_literal,
        } = method_info;
        
        // Use placeholder variant name for stable encoding
        let placeholder_ident = format_ident!("Method{}", method_idx);

        let mut struct_fields = Vec::new();
        let mut destructure_fields = Vec::new();
        let mut argument_idents = Vec::new();
        let mut request_init = Vec::new();

        for field in request_fields {
            let ident = &field.ident;
            let ty = &field.ty;
            struct_fields.push(quote! { pub #ident: #ty });
            destructure_fields.push(quote! { #ident });
            argument_idents.push(quote! { #ident });
            request_init.push(quote! { #ident });
        }

        request_structs.push(quote! {
            #[derive(::bitrpc::bitcode::Encode, ::bitrpc::bitcode::Decode, ::core::fmt::Debug)]
            #vis struct #request_struct_ident {
                #( #struct_fields, )*
            }
        });

        request_variants.push(quote! { #placeholder_ident(#request_struct_ident) });
        response_variants.push(quote! { #placeholder_ident(#success_ty) });
        request_variant_names.push(quote! { #request_ident::#placeholder_ident(_) => #name_literal });
        response_variant_names.push(quote! { #response_ident::#placeholder_ident(_) => #name_literal });

        dispatch_arms.push(quote! {
            #request_ident::#placeholder_ident(payload) => {
                let #request_struct_ident { #( #destructure_fields, )* } = payload;
                match handler.#method_ident(#( #argument_idents ),*).await {
                    ::core::result::Result::Ok(value) => #response_ident::#placeholder_ident(value),
                    ::core::result::Result::Err(err) => #response_ident::Error(err),
                }
            }
        });

        let client_args_def = method_inputs.iter().map(|pat_type| quote! { #pat_type });
        let request_struct_init = quote! {
            #request_struct_ident { #( #request_init, )* }
        };

        client_methods.push(quote! {
            pub async fn #method_ident(&mut self #( , #client_args_def )* ) -> ::bitrpc::Result<#success_ty> {
                let request = #request_ident::#placeholder_ident(#request_struct_init);
                let bytes = ::bitrpc::bitcode::encode(&request);
                let response_bytes = self.transport.call(bytes).await?;
                let response = #response_ident::decode(&response_bytes)?;
                match response {
                    #response_ident::#placeholder_ident(value) => ::core::result::Result::Ok(value),
                    #response_ident::Error(err) => ::core::result::Result::Err(err),
                    other => ::core::result::Result::Err(::bitrpc::RpcError::unexpected(#name_literal, other.variant_name())),
                }
            }
        });
    }
    
    // Add remaining placeholders for future expansion
    for i in methods.len()..(MAX_METHODS - 1) { // -1 to leave room for Error variant
        let placeholder_ident = format_ident!("Placeholder{}", i);
        request_variants.push(quote! { #placeholder_ident });
        response_variants.push(quote! { #placeholder_ident });
        request_variant_names.push(quote! { 
            #request_ident::#placeholder_ident => concat!("Placeholder", stringify!(#i))
        });
        response_variant_names.push(quote! { 
            #response_ident::#placeholder_ident => concat!("Placeholder", stringify!(#i))
        });
    }

    response_variants.push(quote! { Error(#error_path) });
    response_variant_names.push(quote! { #response_ident::Error(_) => "Error" });

    let expanded = quote! {
        #[::bitrpc::async_trait]
        #input

        #( #request_structs )*

        #[derive(::bitrpc::bitcode::Encode, ::bitrpc::bitcode::Decode, ::core::fmt::Debug)]
        #vis enum #request_ident {
            #( #request_variants, )*
        }

        impl #request_ident {
            pub fn encode(&self) -> ::std::vec::Vec<u8> {
                ::bitrpc::bitcode::encode(self)
            }

            pub fn decode(bytes: &[u8]) -> ::core::result::Result<Self, ::bitrpc::DecodeError> {
                ::bitrpc::bitcode::decode(bytes)
            }

            pub fn variant_name(&self) -> &'static str {
                match self {
                    #( #request_variant_names, )*
                }
            }
        }

        #[derive(::bitrpc::bitcode::Encode, ::bitrpc::bitcode::Decode, ::core::fmt::Debug)]
        #vis enum #response_ident {
            #( #response_variants, )*
        }

        impl #response_ident {
            pub fn encode(&self) -> ::std::vec::Vec<u8> {
                ::bitrpc::bitcode::encode(self)
            }

            pub fn decode(bytes: &[u8]) -> ::core::result::Result<Self, ::bitrpc::DecodeError> {
                ::bitrpc::bitcode::decode(bytes)
            }

            pub fn variant_name(&self) -> &'static str {
                match self {
                    #( #response_variant_names, )*
                }
            }
        }

        pub async fn dispatch<T>(handler: &T, request: #request_ident) -> #response_ident
        where
            T: #trait_ident + ?Sized,
        {
            match request {
                #( #dispatch_arms, )*
                _ => #response_ident::Error(#error_path::unknown_method()),
            }
        }

        #vis struct #client_ident<T> {
            transport: T,
        }

        impl<T> #client_ident<T> {
            pub fn new(transport: T) -> Self {
                Self { transport }
            }

            pub fn into_inner(self) -> T {
                self.transport
            }

            pub fn transport(&self) -> &T {
                &self.transport
            }

            pub fn transport_mut(&mut self) -> &mut T {
                &mut self.transport
            }
        }

        impl<T> #client_ident<T> where T: ::bitrpc::RpcTransport {
            #( #client_methods )*
        }

        #[derive(Clone)]
        #vis struct RpcRequestServiceWrapper<T>(pub T);

        impl<T> ::bitrpc::RpcRequestService for RpcRequestServiceWrapper<T>
        where
            T: #trait_ident + Clone,
        {
            type Request = #request_ident;
            type Response = #response_ident;

            async fn dispatch(&self, request: #request_ident) -> #response_ident {
                dispatch(&self.0, request).await
            }
        }
    };

    Ok(expanded)
}

fn ensure_async_trait(trait_item: &mut ItemTrait) -> syn::Result<()> {
    let mut has_send = false;
    let mut has_sync = false;

    for bound in &trait_item.supertraits {
        if let TypeParamBound::Trait(bound_trait) = bound {
            if bound_trait
                .path
                .segments
                .last()
                .map(|seg| seg.ident == "Send")
                .unwrap_or(false)
            {
                has_send = true;
            }

            if bound_trait
                .path
                .segments
                .last()
                .map(|seg| seg.ident == "Sync")
                .unwrap_or(false)
            {
                has_sync = true;
            }
        }
    }

    if !has_send {
        if !trait_item.supertraits.is_empty() {
            trait_item.supertraits.push_punct(syn::token::Plus::default());
        }
        trait_item
            .supertraits
            .push_value(parse_quote!(::core::marker::Send));
    }

    if !has_sync {
        if !trait_item.supertraits.is_empty() {
            trait_item.supertraits.push_punct(syn::token::Plus::default());
        }
        trait_item
            .supertraits
            .push_value(parse_quote!(::core::marker::Sync));
    }

    Ok(())
}

fn collect_methods(trait_item: &ItemTrait) -> syn::Result<Vec<MethodInfo>> {
    let mut methods = Vec::new();

    for item in &trait_item.items {
        match item {
            TraitItem::Fn(method) => {
                if method.default.is_some() {
                    return Err(syn::Error::new(
                        method.sig.span(),
                        "RPC trait methods cannot have default implementations",
                    ));
                }

                if method.sig.asyncness.is_none() {
                    return Err(syn::Error::new(
                        method.sig.span(),
                        "RPC trait methods must be async",
                    ));
                }

                let mut inputs_iter = method.sig.inputs.iter();
                match inputs_iter.next() {
                    Some(FnArg::Receiver(recv)) => {
                        if recv.reference.is_none() || recv.mutability.is_some() {
                            return Err(syn::Error::new(
                                recv.span(),
                                "RPC trait methods must take &self",
                            ));
                        }
                    }
                    _ => {
                        return Err(syn::Error::new(
                            method.sig.span(),
                            "RPC trait methods must take &self",
                        ));
                    }
                }

                let mut request_fields = Vec::new();
                let mut method_inputs = Vec::new();

                for arg in method.sig.inputs.iter().skip(1) {
                    if let FnArg::Typed(pat_type) = arg {
                        if let Pat::Ident(pat_ident) = pat_type.pat.as_ref() {
                            let ident = pat_ident.ident.clone();
                            let ty = (*pat_type.ty).clone();
                            request_fields.push(MethodArg { ident, ty });
                            method_inputs.push(pat_type.clone());
                        } else {
                            return Err(syn::Error::new(
                                pat_type.pat.span(),
                                "RPC trait method arguments must be simple identifiers",
                            ));
                        }
                    } else {
                        return Err(syn::Error::new(
                            arg.span(),
                            "unsupported argument type",
                        ));
                    }
                }

                let success_ty = extract_success_type(&method.sig)?;

                let method_name = method.sig.ident.to_string();
                let variant_base = method_name.to_upper_camel_case();
                let request_struct_ident = format_ident!("{}Request", variant_base);
                let name_literal = LitStr::new(method_name.as_str(), method.sig.ident.span());

                methods.push(MethodInfo {
                    method_ident: method.sig.ident.clone(),
                    request_struct_ident,
                    request_fields,
                    method_inputs,
                    success_ty,
                    name_literal,
                });
            }
            TraitItem::Type(item) => {
                return Err(syn::Error::new(
                    item.span(),
                    "RPC traits cannot declare associated types",
                ));
            }
            TraitItem::Const(item) => {
                return Err(syn::Error::new(
                    item.span(),
                    "RPC traits cannot declare associated constants",
                ));
            }
            _ => {}
        }
    }

    Ok(methods)
}

fn extract_success_type(sig: &syn::Signature) -> syn::Result<Type> {
    let return_type = match &sig.output {
        syn::ReturnType::Default => {
            return Err(syn::Error::new(
                sig.span(),
                "RPC trait methods must return ::bitrpc::Result<T>",
            ))
        }
        syn::ReturnType::Type(_, ty) => ty,
    };

    match return_type.as_ref() {
        Type::Path(type_path) => extract_success_type_from_path(type_path),
        _ => Err(syn::Error::new(
            return_type.span(),
            "RPC trait methods must return ::bitrpc::Result<T>",
        )),
    }
}

fn extract_success_type_from_path(type_path: &syn::TypePath) -> syn::Result<Type> {
    let last_segment = type_path
        .path
        .segments
        .last()
        .ok_or_else(|| syn::Error::new(type_path.span(), "invalid return type"))?;

    if last_segment.ident != "Result" {
        return Err(syn::Error::new(
            last_segment.ident.span(),
            "RPC trait methods must return ::bitrpc::Result<T>",
        ));
    }

    match &last_segment.arguments {
        PathArguments::AngleBracketed(args) => {
            let mut iter = args.args.iter();
            if let Some(syn::GenericArgument::Type(success_ty)) = iter.next() {
                Ok(success_ty.clone())
            } else {
                Err(syn::Error::new(
                    args.span(),
                    "Result must specify a success type",
                ))
            }
        }
        _ => Err(syn::Error::new(
            last_segment.arguments.span(),
            "Result must use angle bracket generic arguments",
        )),
    }
}

fn parse_service_options(
    args: Punctuated<KeyValue, Comma>,
    trait_ident: &Ident,
) -> syn::Result<ServiceOptions> {
    let mut request_ident: Option<Ident> = None;
    let mut response_ident: Option<Ident> = None;
    let mut client_ident: Option<Ident> = None;
    let mut error_path: Option<Path> = None;

    for arg in args {
        let key = arg.key.to_string();
        match key.as_str() {
            "request" => match arg.value {
                Expr::Path(expr_path) if expr_path.path.segments.len() == 1 => {
                    request_ident = Some(expr_path.path.segments[0].ident.clone());
                }
                _ => {
                    return Err(syn::Error::new(
                        arg.value.span(),
                        "request must be a simple identifier",
                    ))
                }
            },
            "response" => match arg.value {
                Expr::Path(expr_path) if expr_path.path.segments.len() == 1 => {
                    response_ident = Some(expr_path.path.segments[0].ident.clone());
                }
                _ => {
                    return Err(syn::Error::new(
                        arg.value.span(),
                        "response must be a simple identifier",
                    ))
                }
            },
            "client" => match arg.value {
                Expr::Path(expr_path) if expr_path.path.segments.len() == 1 => {
                    client_ident = Some(expr_path.path.segments[0].ident.clone());
                }
                _ => {
                    return Err(syn::Error::new(
                        arg.value.span(),
                        "client must be a simple identifier",
                    ))
                }
            },
            "error" => match arg.value {
                Expr::Path(expr_path) => {
                    error_path = Some(expr_path.path.clone());
                }
                _ => {
                    return Err(syn::Error::new(
                        arg.value.span(),
                        "error must be a path",
                    ))
                }
            },
            _ => {
                return Err(syn::Error::new(
                    arg.key.span(),
                    "unsupported service option",
                ))
            }
        }
    }

    let base_name = trait_ident.to_string();
    let request_ident = request_ident.unwrap_or_else(|| format_ident!("{}Request", base_name));
    let response_ident = response_ident.unwrap_or_else(|| format_ident!("{}Response", base_name));
    let client_ident = client_ident.unwrap_or_else(|| format_ident!("{}Client", base_name));
    let error_path = error_path.unwrap_or_else(|| syn::parse_quote!(::bitrpc::RpcError));

    Ok(ServiceOptions {
        request_ident,
        response_ident,
        client_ident,
        error_path,
    })
}
