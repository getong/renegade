//! Macro implementation of the `circuit_type` macro that defines associated types and conversions
//! between them for an application level base type

mod linkable_types;
mod mpc_types;
mod multiprover_circuit_types;
mod secret_share_types;
mod singleprover_circuit_types;

use std::collections::HashSet;

use itertools::Itertools;
use proc_macro::TokenStream;
use proc_macro2::{Ident, Span, TokenStream as TokenStream2};
use quote::ToTokens;
use syn::{
    parse::Parser,
    parse_quote,
    punctuated::Punctuated,
    token::{Brace, Colon, Comma},
    Attribute, Expr, Field, FieldValue, Fields, FieldsNamed, GenericParam, Generics, ItemFn,
    ItemImpl, ItemStruct, Member, Path, Result, Stmt, Token, Type, TypePath,
};

use self::{
    linkable_types::build_linkable_types, mpc_types::build_mpc_types,
    secret_share_types::build_secret_share_types, singleprover_circuit_types::build_circuit_types,
};

/// The trait name for the base type that all other types are derived from
const BASE_TYPE_TRAIT_NAME: &str = "BaseType";

/// The name of the method that converts a serialized scalar iterator to a base type
const FROM_SCALARS_METHOD_NAME: &str = "from_scalars";
/// The name of the method that converts a base type to a serialized vector of scalars
const TO_SCALARS_METHOD_NAME: &str = "to_scalars";
/// The name of the method that converts a base type to a serialized vector of scalars
/// including the scalars needed to commit to the base type in a (possibly) linkable manner
const TO_SCALARS_LINKING_METHOD_NAME: &str = "to_scalars_with_linking";
/// The identifier of the `Scalar` type
const SCALAR_TYPE_IDENT: &str = "Scalar";

/// The method name for creating commitment randomness to a base type
pub(crate) const COMMITMENT_RANDOMNESS_METHOD_NAME: &str = "commitment_randomness";

/// The flag indicating the expansion should include a single prover circuit type definition
/// for the base type
const ARG_SINGLEPROVER_TYPE: &str = "singleprover_circuit";
/// The flag indicating the expansion should include types for an MPC circuit
const ARG_MPC_TYPE: &str = "mpc";
/// The flag indicating the expansion should include types for a multiprover circuit
const ARG_MULTIPROVER_TYPE: &str = "multiprover_circuit";
/// The flag indicating the expansion should include a proof-linkable type
const ARG_LINKABLE_TYPE: &str = "linkable";
/// The flag indicating the expansion should include multiprover linkable types
const ARG_MULTIPROVER_LINKABLE_TYPES: &str = "multiprover_linkable";
/// The flag indicating the expansion should include secret share types
const ARG_SHARE_TYPE: &str = "secret_share";
/// The flag indicating the expansion should include serde derivations
const ARG_SERDE: &str = "serde";

/// The arguments to the `circuit_trace` macro
#[derive(Default)]
pub(crate) struct MacroArgs {
    /// Whether or not to allocate a circuit type for the struct
    pub build_singleprover_types: bool,
    /// Whether or not to allocate linkable commitment types for the struct
    pub build_linkable_types: bool,
    /// Whether or not to allocate MPC circuit types for the struct
    pub build_mpc_types: bool,
    /// Whether or not to allocate multiprover circuit types for the struct
    pub build_multiprover_types: bool,
    /// Whether or not to allocate multiprover linkable circuit types for the struct
    pub build_multiprover_linkable_types: bool,
    /// Whether or not to allocate secret share types for the struct
    pub build_secret_share_types: bool,
    /// Whether or not to include serde derivations for the type
    pub serde: bool,
}

impl MacroArgs {
    /// Validate the argument combinations
    pub fn validate(&self) {
        // A multiprover type must also be a base circuit type
        if self.build_multiprover_types {
            assert!(
                self.build_singleprover_types && self.build_mpc_types,
                "multiprover circuit type requires singleprover and mpc circuit types"
            );
        }

        // A linkable type also requires a circuit base type to be defined
        if self.build_linkable_types {
            assert!(
                self.build_singleprover_types,
                "linkable types require a circuit base type to implement"
            )
        }

        // A multiprover linkable type must also be linkable and a circuit base type
        if self.build_multiprover_linkable_types {
            assert!(
                self.build_singleprover_types && self.build_linkable_types,
                "multiprover linkable types require both circuit base type and base linkable types"
            )
        }

        // A secret share type requires the base type be a single-prover circuit type
        if self.build_secret_share_types {
            assert!(
                self.build_singleprover_types,
                "secret share types require single-prover circuit types"
            )
        }
    }
}

/// Parse macro args from the invocation details
pub(crate) fn parse_macro_args(args: TokenStream) -> Result<MacroArgs> {
    let mut macro_args = MacroArgs::default();
    let parsed_args =
        Punctuated::<Ident, Comma>::parse_terminated.parse2(TokenStream2::from(args))?;

    for arg in parsed_args.iter() {
        match arg.to_string().as_str() {
            ARG_SINGLEPROVER_TYPE => macro_args.build_singleprover_types = true,
            ARG_LINKABLE_TYPE => macro_args.build_linkable_types = true,
            ARG_MPC_TYPE => macro_args.build_mpc_types = true,
            ARG_MULTIPROVER_TYPE => macro_args.build_multiprover_types = true,
            ARG_MULTIPROVER_LINKABLE_TYPES => macro_args.build_multiprover_linkable_types = true,
            ARG_SHARE_TYPE => macro_args.build_secret_share_types = true,
            ARG_SERDE => macro_args.serde = true,
            unknown => panic!("received unexpected argument {unknown}"),
        }
    }

    macro_args.validate();
    Ok(macro_args)
}

// -------------------
// | Core Macro Impl |
// -------------------

/// Implementation of the type derivation macro
pub(crate) fn circuit_type_impl(target_struct: ItemStruct, macro_args: MacroArgs) -> TokenStream {
    // Copy the existing struct into the result
    let mut out_tokens = TokenStream2::default();
    out_tokens.extend(target_struct.to_token_stream());

    // Build the implementation of the `BaseType` trait
    out_tokens.extend(build_base_type_impl(&target_struct));

    // Build singleprover circuit types
    if macro_args.build_singleprover_types {
        let circuit_type_tokens = build_circuit_types(&target_struct, macro_args.serde);
        out_tokens.extend(circuit_type_tokens);
    }

    // Build MPC types
    if macro_args.build_mpc_types {
        let mpc_type_tokens = build_mpc_types(
            &target_struct,
            macro_args.build_multiprover_types,
            false, /* multiprover_base_only */
        );
        out_tokens.extend(mpc_type_tokens);
    }

    // Build the commitment-linkable type
    if macro_args.build_linkable_types {
        let linkable_type_tokens = build_linkable_types(
            &target_struct,
            macro_args.build_multiprover_linkable_types,
            macro_args.serde,
        );
        out_tokens.extend(linkable_type_tokens);
    }

    // Build secret share types
    if macro_args.build_secret_share_types {
        let secret_share_type_tokens = build_secret_share_types(&target_struct, macro_args.serde);
        out_tokens.extend(secret_share_type_tokens);
    }

    out_tokens.into()
}

// ---------------------------
// | BaseType Implementation |
// ---------------------------

/// Build the `impl BaseType` block
fn build_base_type_impl(base_type: &ItemStruct) -> TokenStream2 {
    let trait_ident = new_ident(BASE_TYPE_TRAIT_NAME);
    let generics = base_type.generics.clone();
    let where_clause = generics.where_clause.clone();

    let base_type_ident = base_type.ident.clone();
    let base_type_params = params_from_generics(generics.clone());
    let scalar_type_path = path_from_ident(new_ident(SCALAR_TYPE_IDENT));

    let from_scalars_impl = build_deserialize_method(
        new_ident(FROM_SCALARS_METHOD_NAME),
        scalar_type_path.clone(),
        path_from_ident(trait_ident.clone()),
        base_type,
    );

    let to_scalars_impl = build_serialize_method(
        new_ident(TO_SCALARS_METHOD_NAME),
        scalar_type_path.clone(),
        base_type,
    );

    let to_scalars_linking_impl = build_serialize_method(
        new_ident(TO_SCALARS_LINKING_METHOD_NAME),
        scalar_type_path,
        base_type,
    );

    let impl_block: ItemImpl = parse_quote! {
        impl #generics #trait_ident for #base_type_ident <#base_type_params>
            #where_clause
        {
            #from_scalars_impl
            #to_scalars_impl
            #to_scalars_linking_impl
        }
    };
    impl_block.to_token_stream()
}

// -----------
// | Helpers |
// -----------

/// A helper that specifies the default call site span for an Identifier
fn new_ident(name: &str) -> Ident {
    Ident::new(name, Span::call_site())
}

/// A helper that creates an identifier with the given prefix
fn ident_with_prefix(original: &str, prefix: &str) -> Ident {
    new_ident(&format!("{prefix}{original}"))
}

/// A helper to strip a prefix from an identifier and return a new identifier
fn ident_strip_prefix(original: &str, prefix: &str) -> Ident {
    let stripped = original.strip_prefix(prefix).unwrap_or(original);
    new_ident(stripped)
}

/// A helper that creates an identifier with the given suffix
fn ident_with_suffix(original: &str, suffix: &str) -> Ident {
    new_ident(&format!("{original}{suffix}"))
}

/// A helper to strip a suffix from an identifier and return a new identifier
fn ident_strip_suffix(original: &str, suffix: &str) -> Ident {
    let stripped = original.strip_suffix(suffix).unwrap_or(original);
    new_ident(stripped)
}

/// Convert a string to a `Path` syntax tree object representing a type path
fn str_to_path(s: &str) -> Path {
    path_from_ident(new_ident(s))
}

/// Convert an `Ident` directly into a `Path`
fn path_from_ident(identifier: Ident) -> Path {
    parse_quote!(#identifier)
}

/// Add generic parameters to an identifier
fn ident_with_generics(ident: Ident, generics: Generics) -> Path {
    let params = params_from_generics(generics);
    parse_quote!(#ident <#params>)
}

/// Get the identifiers of a given set of generics
fn params_from_generics(generics: Generics) -> Punctuated<Ident, Comma> {
    let mut res = Punctuated::new();
    for generic in generics.params.into_iter() {
        match generic {
            GenericParam::Type(type_param) => res.push(type_param.ident),
            GenericParam::Const(const_generic) => res.push(const_generic.ident),
            GenericParam::Lifetime(_) => panic!("implement lifetime generic support"),
        }
    }

    res
}

/// Merge two sets of `Generics`
fn merge_generics(mut generics1: Generics, generics2: Generics) -> Generics {
    // Combine the params, deduplicating between the sets of generics
    let generic_params: HashSet<Ident> = params_from_generics(generics1.clone())
        .into_iter()
        .collect();
    generics1
        .params
        .extend(generics2.params.into_iter().filter(|param| match param {
            GenericParam::Type(type_param) => !generic_params.contains(&type_param.ident),
            GenericParam::Const(const_param) => !generic_params.contains(&const_param.ident),
            _ => true, // Ignore lifetime params
        }));

    // Combine the where clauses
    let mut generics1_predicates = generics1
        .where_clause
        .map(|where_clause| where_clause.predicates)
        .unwrap_or_default();
    let generics2_predicates = generics2
        .where_clause
        .map(|where_clause| where_clause.predicates)
        .unwrap_or_default();

    generics1_predicates.extend(generics2_predicates);
    generics1.where_clause = Some(parse_quote!(where #generics1_predicates));
    generics1
}

/// Remove the second set of generics from the first
fn filter_generics(base: Generics, filter: Generics) -> Generics {
    // Remove the params from the base
    let filter_params: HashSet<Ident> = params_from_generics(filter).into_iter().collect();
    let new_base_params: Punctuated<GenericParam, Comma> = base
        .params
        .clone()
        .into_iter()
        .filter(|param| match param {
            GenericParam::Type(type_param) => !filter_params.contains(&type_param.ident),
            GenericParam::Const(const_param) => !filter_params.contains(&const_param.ident),
            _ => true, // Ignore lifetime params
        })
        .collect();

    let mut new_generics = base;
    new_generics.params = new_base_params;
    new_generics
}

/// Implements a serialization function that looks like
///     fn #method_name(self) -> Vec<#target_type> {
///         vec![self.field1, self.field2, ...]
///     }
fn build_serialize_method(
    method_name: Ident,
    target_type: Path,
    self_struct: &ItemStruct,
) -> TokenStream2 {
    let mut field_exprs: Vec<Stmt> = Vec::with_capacity(self_struct.fields.len());
    for field in self_struct.fields.iter().cloned() {
        let field_ident = field.ident;
        field_exprs.push(parse_quote! {
            res.extend(self.#field_ident.#method_name());
        });
    }

    let fn_impl: ItemFn = parse_quote! {
        fn #method_name(&self) -> Vec<#target_type> {
            let mut res = Vec::new();
            #(#field_exprs)*

            res
        }
    };
    fn_impl.to_token_stream()
}

/// Implements a deserialization function for a trait that looks like the following
///     fn #method_name<I: Iterator<Item = #from_type>>(i: &mut I) -> Self {
///         Self { field1: i.next().unwrap(), field2: , ... }
///     }
fn build_deserialize_method(
    method_name: Ident,
    from_type: Path,
    trait_ident: Path,
    self_struct: &ItemStruct,
) -> TokenStream2 {
    let mut fields_expr: Punctuated<FieldValue, Comma> = Punctuated::new();
    for field in self_struct.fields.iter().cloned() {
        let ident = field.ident.expect("only named fields supported");
        let field_type = field.ty;

        // The parse field expr recursively calls `#method_name` on the field type
        let parse_field_expr: Expr = parse_quote! {
            <#field_type as #trait_ident>::#method_name(i)
        };

        fields_expr.push(FieldValue {
            attrs: Vec::new(),
            member: Member::Named(ident),
            colon_token: Some(Colon::default()),
            expr: parse_field_expr,
        });
    }

    parse_quote! {
        fn #method_name<I: Iterator<Item = #from_type>>(i: &mut I) -> Self {
            Self {
                #fields_expr
            }
        }
    }
}

/// Build an implementation of the `commitment_randomness` method that calls out to each
/// field's implementation
pub(crate) fn build_commitment_randomness_method(
    base_type: &ItemStruct,
    from_trait: Path,
) -> TokenStream2 {
    // Build the body of the `commitment_randomness` method
    let commitment_randomness_ident = new_ident(COMMITMENT_RANDOMNESS_METHOD_NAME);
    let mut field_stmts: Vec<Stmt> = Vec::new();
    for field in base_type.fields.iter() {
        let field_ident = field.ident.clone();
        let field_type = field.ty.clone();
        field_stmts.push(parse_quote! {
            res.extend(<#field_type as #from_trait>::#commitment_randomness_ident(&self.#field_ident, r));
        });
    }

    let fn_def: ItemFn = parse_quote! {
        fn #commitment_randomness_ident <R: RngCore + CryptoRng>(&self, r: &mut R) -> Vec<Scalar> {
            let mut res = Vec::new();
            #(#field_stmts)*

            res
        }
    };
    fn_def.to_token_stream()
}

/// Implement `Clone` by cloning each field individually, this is useful when we have a generic
/// that does not extend clone, i.e. the `MpcNetwork`, but we still want its type to be `Clone`
fn impl_clone_by_fields(base_struct: &ItemStruct) -> TokenStream2 {
    let generics = base_struct.generics.clone();
    let where_clause = generics.where_clause.clone();
    let base_type_ident = base_struct.ident.clone();
    let base_type_with_generics = ident_with_generics(base_type_ident.clone(), generics.clone());

    let mut field_exprs: Punctuated<FieldValue, Comma> = Punctuated::new();
    for field in base_struct.fields.iter() {
        let field_name = new_ident(&field.ident.clone().unwrap().to_string());
        field_exprs.push(parse_quote! (#field_name: self.#field_name.clone()));
    }

    let impl_block: ItemImpl = parse_quote! {
        impl #generics Clone for #base_type_with_generics
            #where_clause
        {
            fn clone(&self) -> Self {
                #base_type_ident {
                    #field_exprs
                }
            }
        }
    };
    impl_block.to_token_stream()
}

/// Build a `serde` serialization and deserialization implementation for the type
fn build_serde_methods(
    base_type: &ItemStruct,
    serialized_type: Path,
    serialize_method: Ident,
    deserialize_method: Ident,
) -> TokenStream2 {
    let generics = base_type.generics.clone();
    let where_clause = base_type.generics.where_clause.clone();

    let mut deserialize_generics = generics.clone();
    deserialize_generics.params.push(parse_quote!('de));

    let base_type_ident = base_type.ident.clone();
    let base_type_with_generics = ident_with_generics(base_type_ident, generics.clone());

    let serialize_impl: ItemImpl = parse_quote! {
        impl #generics serde::Serialize for #base_type_with_generics
            #where_clause
        {
            fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                self.#serialize_method().serialize(serializer)
            }
        }
    };

    let deserialize_impl: ItemImpl = parse_quote! {
        impl #deserialize_generics serde::Deserialize<'de> for #base_type_with_generics
            #where_clause
        {
            fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                let mut res = <Vec<#serialized_type>>::deserialize(deserializer)?;
                Ok(Self::#deserialize_method(&mut res.into_iter()))
            }
        }
    };

    let mut res = serialize_impl.to_token_stream();
    res.extend(deserialize_impl.to_token_stream());
    res
}

/// Build a replica of the given struct with the given modifications, using an
/// implemented trait's associated type as the new type for each field
fn build_modified_struct_from_associated_types(
    base_type: &ItemStruct,
    new_name: Ident,
    attributes: Vec<Attribute>,
    generics: Generics,
    type_derivation_trait_ident: Path,
    associated_type_ident: Path,
) -> ItemStruct {
    // Build the fields fo the var struct
    let new_fields = base_type
        .fields
        .iter()
        .map(|f| {
            let name = f.ident.clone();
            let curr_type = f.ty.clone();

            // Construct the fully-qualified path type expression
            let base_trait = type_derivation_trait_ident.clone();
            let associated = associated_type_ident.clone();
            let type_path: TypePath = parse_quote!(
                <#curr_type as #base_trait>::#associated
            );

            Field {
                vis: f.vis.clone(),
                attrs: Vec::new(),
                ident: name,
                colon_token: f.colon_token,
                ty: Type::Path(type_path),
            }
        })
        .collect_vec();

    let mut named = Punctuated::<Field, Comma>::new();
    for field in new_fields.into_iter() {
        named.push(field);
    }

    let named_fields = FieldsNamed {
        brace_token: Brace::default(),
        named,
    };

    ItemStruct {
        attrs: attributes,
        vis: base_type.vis.clone(),
        struct_token: Token![struct](Span::call_site()),
        ident: new_name,
        generics,
        fields: Fields::Named(named_fields),
        semi_token: None,
    }
}