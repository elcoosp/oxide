//! The V2-F08 proc macros. The runtime behavior lives in
//! `oxide-rustfn` ([`oxide_rustfn::shim::run_shim`],
//! [`oxide_rustfn::types`]); these macros only parse and emit:
//!
//! - [`server_fn`]: keeps the user fn untouched and generates, next
//!   to it, the C-ABI shim (`__oxide_shim_<name>`), the entry
//!   accessor (`__oxide_entry_<name>`) and the types record
//!   (`__oxide_types_<name>`).
//! - [`TsType`]: the derive behind `#[derive(TsType)]` — named-field
//!   structs and fieldless enums (the substrate limit, documented).
//! - [`export_table`]: the crate-level `oxide_fn_table` +
//!   `oxide_fn_types_json` exports the host loads.

use proc_macro::TokenStream;
use proc_macro2::{Literal, Span};
use quote::{format_ident, quote};
use syn::parse_macro_input;
use syn::punctuated::Punctuated;
use syn::spanned::Spanned;
use syn::token::Comma;
use syn::{Data, DeriveInput, Fields, ItemFn, PathArguments, Type};

/// Wrap a `pub fn name(input: In) -> Result<Out, E>` for the native
/// server-function boundary. The function stays exactly as written
/// (call it natively from the same crate); the generated shim decodes
/// the ODF input, calls it, catches panics and encodes the result —
/// the contract [`oxide_rustfn::shim::run_shim`] owns.
///
/// The body must be sync (the host runs it on blocking threads) and
/// return `Result<Ok, E: Display>`.
#[proc_macro_attribute]
pub fn server_fn(attr: TokenStream, item: TokenStream) -> TokenStream {
    if !attr.is_empty() {
        return syn::Error::new(Span::call_site(), "`#[server_fn]` takes no arguments")
            .to_compile_error()
            .into();
    }
    let f = parse_macro_input!(item as ItemFn);
    let name = &f.sig.ident;
    let name_str = name.to_string();

    // One typed input or none, and a `Result<Ok, _>` return — the two
    // shapes the shim's decode/call/encode closure can spell.
    let shapes = match server_fn_shapes(&f) {
        Ok(shapes) => shapes,
        Err(err) => return err.to_compile_error().into(),
    };

    // The call inside the shim closure: decode the ODF input, run the
    // function, encode the result — every failure flattened to the
    // message string `run_shim` turns into the status-1 payload.
    let call = if let Some(ty) = shapes.input_ty {
        quote! {
            let args: #ty = match oxide_rustfn::bridge::decode_odf(bytes) {
                Ok(args) => args,
                Err(e) => return Err(e.to_string()),
            };
            match #name(args) {
                Ok(value) => oxide_rustfn::bridge::encode_odf(&value).map_err(|e| e.to_string()),
                Err(e) => Err(e.to_string()),
            }
        }
    } else {
        quote! {
            let _ = bytes;
            match #name() {
                Ok(value) => oxide_rustfn::bridge::encode_odf(&value).map_err(|e| e.to_string()),
                Err(e) => Err(e.to_string()),
            }
        }
    };

    let (input_annots, input_decls) = match shapes.input_ty {
        Some(ty) => (
            quote! { vec![<#ty as oxide_rustfn::types::TsType>::ts_type_name()] },
            quote! { let mut decls = <#ty as oxide_rustfn::types::TsType>::ts_decls(); },
        ),
        None => (
            quote! { Vec::new() },
            quote! { let mut decls = Vec::new(); },
        ),
    };

    let names = GeneratedNames {
        shim: format_ident!("__oxide_shim_{name}"),
        entry: format_ident!("__oxide_entry_{name}"),
        types: format_ident!("__oxide_types_{name}"),
        name_str,
    };
    emit_server_fn_expansion(&f, &names, &shapes, &call, &input_annots, &input_decls)
}

/// The input/output shapes `server_fn` accepts: one typed input or
/// none, and a `Result<Ok, _>` return (the shim maps `Ok` to status 0
/// and `Err: Display` to status 1).
struct ServerFnShapes<'a> {
    input_ty: Option<&'a Type>,
    ok_ty: &'a Type,
}

/// The generated item idents (the shim, the entry accessor, the types
/// record) and the source name they carry.
struct GeneratedNames {
    shim: proc_macro2::Ident,
    entry: proc_macro2::Ident,
    types: proc_macro2::Ident,
    name_str: String,
}

/// Validate a `#[server_fn]` function's signature: sync, one typed
/// input or none, `Result<Ok, E>` return. Every refusal names the fix.
fn server_fn_shapes(f: &ItemFn) -> Result<ServerFnShapes<'_>, syn::Error> {
    if let Some(asyncness) = &f.sig.asyncness {
        return Err(syn::Error::new(
            asyncness.span(),
            "`#[server_fn]` bodies are sync — the host runs them on blocking threads",
        ));
    }
    let mut inputs = f.sig.inputs.iter();
    let input_ty: Option<&Type> = match inputs.next() {
        None => None,
        Some(syn::FnArg::Typed(pat)) => {
            if inputs.next().is_some() {
                return Err(syn::Error::new(
                    f.sig.inputs.span(),
                    "`#[server_fn]` takes one input argument or none",
                ));
            }
            Some(&pat.ty)
        }
        Some(syn::FnArg::Receiver(r)) => {
            return Err(syn::Error::new(
                r.span(),
                "`#[server_fn]` does not support methods",
            ));
        }
    };
    let ok_ty: &Type = match &f.sig.output {
        syn::ReturnType::Default => {
            return Err(syn::Error::new(
                f.sig.output.span(),
                "`#[server_fn]` must return `Result<Out, E>`",
            ));
        }
        syn::ReturnType::Type(_, ty) => match result_ok_ty(ty) {
            Some(ok) => ok,
            None => {
                return Err(syn::Error::new(
                    ty.span(),
                    "`#[server_fn]` must return `Result<Out, E>`",
                ));
            }
        },
    };
    Ok(ServerFnShapes { input_ty, ok_ty })
}

/// Emit the three generated items beside the untouched function: the
/// C-ABI shim (input bytes in, the output buffer filled, the status
/// returned), the entry accessor (the name + shim pair the
/// `export_table!` static carries) and the types record (the inline
/// annotations and declaration blocks `oxide_fn_types_json`
/// serializes).
fn emit_server_fn_expansion(
    f: &ItemFn,
    names: &GeneratedNames,
    shapes: &ServerFnShapes<'_>,
    call: &proc_macro2::TokenStream,
    input_annots: &proc_macro2::TokenStream,
    input_decls: &proc_macro2::TokenStream,
) -> TokenStream {
    let GeneratedNames {
        shim: shim_ident,
        entry: entry_ident,
        types: types_ident,
        name_str,
    } = names;
    let ServerFnShapes { input_ty: _, ok_ty } = shapes;
    let ok_ty = *ok_ty;
    quote! {
        #f

        /// The generated C-ABI shim: input bytes in, the output buffer
        /// filled, the status returned (0 ok / 1 error / 2 panic). The
        /// item is a safe `extern "C"` fn — the raw-pointer work sits
        /// in its own guarded blocks and the panics are caught inside
        /// `run_shim`, so nothing unwinds across the boundary.
        #[doc(hidden)]
        pub extern "C" fn #shim_ident(
            inp: *const u8,
            len: usize,
            out: *mut oxide_rustfn::abi::RawBuf,
        ) -> i32 {
            let input: &[u8] = if inp.is_null() || len == 0 {
                &[]
            } else {
                // SAFETY: the host passes a live `len`-byte buffer.
                unsafe { std::slice::from_raw_parts(inp, len) }
            };
            let mut buf = oxide_rustfn::abi::RawBuf::empty();
            let status = oxide_rustfn::shim::run_shim(input, &mut buf, |bytes| {
                #call
            });
            // SAFETY: the host always passes a writable slot.
            if !out.is_null() {
                unsafe { *out = buf };
            }
            status
        }

        /// The generated entry: the name + shim pair the
        /// `export_table!` static (or a hand-built table) carries.
        #[doc(hidden)]
        #[must_use]
        pub fn #entry_ident() -> oxide_rustfn::abi::FnEntry {
            oxide_rustfn::abi::FnEntry {
                name: oxide_rustfn::abi::leak_name(#name_str),
                call: #shim_ident,
            }
        }

        /// The generated types record: the inline annotations and
        /// declaration blocks the `oxide_fn_types_json` export
        /// serializes.
        #[doc(hidden)]
        #[must_use]
        pub fn #types_ident() -> oxide_rustfn::types::FnTypeEntry {
            #input_decls
            decls.extend(<#ok_ty as oxide_rustfn::types::TsType>::ts_decls());
            oxide_rustfn::types::FnTypeEntry {
                name: #name_str.to_string(),
                file: file!().to_string(),
                args: #input_annots,
                ret: <#ok_ty as oxide_rustfn::types::TsType>::ts_type_name(),
                decls,
            }
        }
    }
    .into()
}

/// The `Ok` half of a `Result<Ok, E>` return type (syntactic: the last
/// path segment must be `Result` with at least one angle argument).
fn result_ok_ty(ty: &Type) -> Option<&Type> {
    let path = match ty {
        Type::Path(p) if p.qself.is_none() => &p.path,
        _ => return None,
    };
    let seg = path.segments.last()?;
    if seg.ident != "Result" {
        return None;
    }
    match &seg.arguments {
        PathArguments::AngleBracketed(args) => match args.args.first()? {
            syn::GenericArgument::Type(t) => Some(t),
            _ => None,
        },
        _ => None,
    }
}

/// Generate the `oxide_rustfn::types::TsType` impl: named-field
/// structs render an `export interface`, fieldless enums an
/// `export type` union. Nested field types must implement [`TsType`]
/// too (their declaration blocks ride the parent's) — the primitives
/// implement it already.
///
/// # Panics
///
/// Panics only on the macro's own malformed synthesis: a named-field
/// struct whose field ident is somehow absent (`expect("named field")`)
/// — unreachable for input that parsed as `Fields::Named`.
#[proc_macro_derive(TsType)]
pub fn ts_type_derive(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    let ident = &input.ident;
    let name_str = ident.to_string();

    let (self_block, nested): (String, Vec<&Type>) = match &input.data {
        Data::Struct(data) => match &data.fields {
            Fields::Named(named) => {
                let mut lines = Vec::new();
                let mut nested = Vec::new();
                for f in &named.named {
                    let fname = f.ident.as_ref().expect("named field");
                    let ts = type_to_ts(&f.ty);
                    lines.push(format!("{fname}: {ts}"));
                    nested.push(&f.ty);
                }
                (
                    format!("export interface {name_str} {{ {} }}", lines.join("; ")),
                    nested,
                )
            }
            other => {
                return syn::Error::new(
                    other.span(),
                    "`#[derive(TsType)]` supports named-field structs (the V2-F08 substrate limit)",
                )
                .to_compile_error()
                .into();
            }
        },
        Data::Enum(data) if data.variants.iter().all(|v| v.fields.is_empty()) => {
            let names: Vec<String> = data
                .variants
                .iter()
                .map(|v| format!("'{}'", v.ident))
                .collect();
            (
                format!("export type {name_str} = {};", names.join(" | ")),
                Vec::new(),
            )
        }
        _ => {
            return syn::Error::new(
                input.ident.span(),
                "`#[derive(TsType)]` supports named-field structs and fieldless enums (the V2-F08 substrate limit)",
            )
            .to_compile_error()
            .into();
        }
    };

    let nested_extends = nested.iter().map(|ty| {
        quote! {
            out.extend(<#ty as oxide_rustfn::types::TsType>::ts_decls());
        }
    });

    quote! {
        impl oxide_rustfn::types::TsType for #ident {
            fn ts_type_name() -> String {
                #name_str.to_string()
            }
            fn ts_decls() -> Vec<(String, String)> {
                let mut out = Vec::new();
                out.push((#name_str.to_string(), #self_block.to_string()));
                #(#nested_extends)*
                out
            }
        }
    }
    .into()
}

/// The crate-level exports the host loads (spec §V2-F08 verbatim):
/// `oxide_fn_table` (the `FnTable` static — the ABI version, the entry
/// array and the free function, all const) and the optional
/// `oxide_fn_types_json` (the types payload, NUL-terminated, `""` when
/// serialization fails). `export_table!(thumbnail, scale)` — the
/// names must be `#[server_fn]` exports in the same module.
#[proc_macro]
pub fn export_table(input: TokenStream) -> TokenStream {
    let names = parse_macro_input!(input with Punctuated::<syn::Ident, Comma>::parse_terminated);
    let count = names.len();

    let entries = names.iter().map(|name| {
        let shim = format_ident!("__oxide_shim_{}", name);
        let c_name = match std::ffi::CString::new(name.to_string()) {
            Ok(c) => Literal::c_string(&c),
            Err(_) => {
                return syn::Error::new(name.span(), "the export name contains an interior NUL")
                    .to_compile_error();
            }
        };
        quote! {
            oxide_rustfn::abi::FnEntry {
                name: #c_name.as_ptr(),
                call: #shim,
            }
        }
    });

    let types_calls = names.iter().map(|name| {
        let types_ident = format_ident!("__oxide_types_{}", name);
        quote! { #types_ident() }
    });

    quote! {
        /// The entry array the table points at (const: the names are
        /// C-string literals, the shims fn pointers).
        #[doc(hidden)]
        static __OXIDE_FN_ENTRIES: [oxide_rustfn::abi::FnEntry; #count] = [
            #(#entries),*
        ];

        /// The free the table carries: the shim allocations come from
        /// this crate's allocator, so the release happens here (the
        /// host hands the buffer back through this pointer). A safe
        /// `extern "C"` fn — the reconstruction sits in its own
        /// guarded block.
        #[doc(hidden)]
        extern "C" fn __oxide_free_fn(buf: oxide_rustfn::abi::RawBuf) {
            if buf.ptr.is_null() || buf.cap == 0 {
                return;
            }
            // SAFETY: every buffer the shims hand out came from
            // `RawBuf::alloc` on this crate's global allocator.
            drop(unsafe { std::vec::Vec::from_raw_parts(buf.ptr, buf.len, buf.cap) });
        }

        /// The spec-verbatim table: ABI version, entries, free.
        #[doc(hidden)]
        static __OXIDE_TABLE: oxide_rustfn::abi::FnTable = oxide_rustfn::abi::FnTable {
            abi_version: oxide_rustfn::abi::ABI_VERSION,
            len: #count,
            entries: __OXIDE_FN_ENTRIES.as_ptr(),
            free: __oxide_free_fn,
        };

        /// The host's loader entry (§V2-F08 verbatim export name).
        #[unsafe(no_mangle)]
        pub extern "C" fn oxide_fn_table() -> *const oxide_rustfn::abi::FnTable {
            &__OXIDE_TABLE
        }

        /// The optional types side export: the NUL-terminated JSON the
        /// host renders into `.oxide/types/rust-fns.d.ts` (a missing
        /// or empty payload reads as absent).
        #[unsafe(no_mangle)]
        pub extern "C" fn oxide_fn_types_json() -> *const std::os::raw::c_char {
            use std::sync::OnceLock;
            static TYPES: OnceLock<std::ffi::CString> = OnceLock::new();
            TYPES
                .get_or_init(|| {
                    let entries = [#(#types_calls),*];
                    std::ffi::CString::new(oxide_rustfn::types::types_json(&entries))
                        .unwrap_or_default()
                })
                .as_ptr()
        }
    }
    .into()
}

/// The syntactic Rust → TS mapping the derive uses for field types.
fn type_to_ts(ty: &Type) -> String {
    match ty {
        Type::Reference(r) => type_to_ts(&r.elem),
        Type::Paren(p) => type_to_ts(&p.elem),
        Type::Group(g) => type_to_ts(&g.elem),
        Type::Array(a) => format!("Array<{}>", type_to_ts(&a.elem)),
        Type::Tuple(tuple) => format!(
            "[{}]",
            tuple
                .elems
                .iter()
                .map(type_to_ts)
                .collect::<Vec<_>>()
                .join(", ")
        ),
        Type::Path(p) => path_to_ts(&p.path),
        _ => "unknown".to_string(),
    }
}

fn path_to_ts(path: &syn::Path) -> String {
    let Some(seg) = path.segments.last() else {
        return "unknown".to_string();
    };
    let name = seg.ident.to_string();
    let args: Vec<String> = match &seg.arguments {
        PathArguments::AngleBracketed(a) => a
            .args
            .iter()
            .filter_map(|arg| match arg {
                syn::GenericArgument::Type(t) => Some(type_to_ts(t)),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    };
    match name.as_str() {
        "String" | "str" | "char" | "PathBuf" | "OsString" | "Cow" => "string".to_string(),
        "i8" | "i16" | "i32" | "i64" | "i128" | "isize" | "u8" | "u16" | "u32" | "u64" | "u128"
        | "usize" | "f32" | "f64" | "NonZeroU8" | "NonZeroU16" | "NonZeroU32" | "NonZeroU64"
        | "Number" => "number".to_string(),
        "bool" => "boolean".to_string(),
        "Value" => "unknown".to_string(),
        "Option" => args
            .first()
            .map_or_else(|| "unknown".to_string(), |t| format!("{t} | null")),
        "Vec" | "HashSet" | "BTreeSet" | "VecDeque" => args
            .first()
            .map_or_else(|| "unknown".to_string(), |t| format!("Array<{t}>")),
        "HashMap" | "BTreeMap" => args
            .get(1)
            .map_or_else(|| "unknown".to_string(), |t| format!("Record<string, {t}>")),
        "Box" | "Arc" | "Rc" | "Ref" | "RefMut" => args
            .first()
            .cloned()
            .unwrap_or_else(|| "unknown".to_string()),
        // A custom type: the annotation is its name and its declaration
        // block rides the parent via its own `TsType` impl.
        _ => name,
    }
}
