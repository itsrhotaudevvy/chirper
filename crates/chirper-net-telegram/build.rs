//! Build script: parses the TL schema in `tl/` and writes `$OUT_DIR/tl_generated.rs`.
//!
//! This is the crate's own TL parser and code generator, written against std only. It emits:
//! - `types::<ns>::<Name>`: a struct per constructor, with `Identifiable`, `Serializable` and
//!   `Deserializable` (plus an inherent `read_body` that reads the fields without the id);
//! - `enums::<ns>::<Type>`: an enum per boxed type, one variant per constructor;
//! - `functions::<ns>::<Name>`: a struct per function, implementing `RemoteCall`.
//!
//! Every constructor of `mtproto.tl` is emitted, plus the functions named in `tl/allowlist.txt`
//! and the transitive closure of the types they mention. Constructor ids come from the `#hex` in
//! the schema, or from the CRC32 of the normalised line when the schema gives none.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::{env, fs};

/// Built-in constructors the generator does not emit: the core types and the types written by hand.
const SKIPPED: &[&str] = &[
    "int",
    "long",
    "double",
    "string",
    "bytes",
    "vector",
    "int128",
    "int256",
    "boolFalse",
    "boolTrue",
    "true",
];

const KEYWORDS: &[&str] = &[
    "as", "async", "await", "break", "const", "continue", "dyn", "else", "enum", "extern", "false",
    "fn", "for", "if", "impl", "in", "let", "loop", "match", "mod", "move", "mut", "pub", "ref",
    "return", "static", "struct", "trait", "true", "type", "unsafe", "use", "where", "while",
    "abstract", "become", "box", "do", "final", "macro", "override", "priv", "typeof", "unsized",
    "virtual", "yield", "try", "gen",
];

/// A TL type expression as it appears in a parameter or a result.
#[derive(Clone, Debug, PartialEq)]
enum Ty {
    Int,
    Long,
    Double,
    Int128,
    Int256,
    Bytes,
    Str,
    Bool,
    /// The `true` flag-only type: presence is carried by a flags bit, no bytes.
    TrueFlag,
    /// `Object`: an arbitrary serialised object, kept as raw bytes.
    Object,
    /// The `#` flags word.
    Flags,
    /// Boxed `Vector<T>`: constructor id, count, items.
    Vector(Box<Ty>),
    /// Bare `vector<T>`: count and items.
    BareVector(Box<Ty>),
    /// A boxed type, by full schema name (`help.Config`).
    Named(String),
    /// `%T`: the only constructor of `T`, serialised without its id. Resolved to `Bare` after parsing.
    BareType(String),
    /// A constructor used directly (lowercase in the schema), serialised without its id.
    Bare(String),
    /// A generic parameter of a function (`X`, or `!X` for a query).
    Generic(String),
    /// `flags.N?T`: present when bit N of the named flags word is set.
    Opt {
        flags: String,
        bit: u32,
        inner: Box<Ty>,
    },
}

#[derive(Clone, Debug)]
struct Param {
    name: String,
    ty: Ty,
}

#[derive(Clone, Debug)]
struct Ctor {
    /// Full schema name, for example `help.getConfig`.
    name: String,
    id: u32,
    params: Vec<Param>,
    generics: Vec<String>,
    result: Ty,
    function: bool,
}

#[derive(Default)]
struct Schema {
    ctors: BTreeMap<String, Ctor>,
    /// Boxed type name to the full names of its constructors.
    types: BTreeMap<String, Vec<String>>,
}

fn main() {
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR"));
    for file in ["tl/api.tl", "tl/mtproto.tl", "tl/allowlist.txt", "build.rs"] {
        println!("cargo:rerun-if-changed={file}");
    }
    match generate(&manifest.join("tl")) {
        Ok(source) => {
            fs::write(out_dir.join("tl_generated.rs"), source).expect("write tl_generated.rs");
        }
        Err(err) => panic!("TL code generation failed: {err}"),
    }
}

fn generate(tl: &Path) -> Result<String, String> {
    let read = |name: &str| -> Result<String, String> {
        fs::read_to_string(tl.join(name)).map_err(|e| format!("reading {name}: {e}"))
    };
    let mtproto = read("mtproto.tl")?;
    let api = read("api.tl")?;
    let allow = read("allowlist.txt")?;

    let mut schema = Schema::default();
    let mut mtproto_names = Vec::new();
    for (source, is_mtproto) in [(&mtproto, true), (&api, false)] {
        let mut functions = false;
        for raw in source.lines() {
            let line = match raw.trim().split_once("//") {
                Some((before, _)) if before.trim().is_empty() => continue,
                Some((before, _)) => before.trim(),
                None => raw.trim(),
            };
            if line.is_empty() {
                continue;
            }
            match line {
                "---functions---" => {
                    functions = true;
                    continue;
                }
                "---types---" => {
                    functions = false;
                    continue;
                }
                _ => {}
            }
            if let Some(ctor) = parse_definition(line, functions)? {
                if is_mtproto {
                    mtproto_names.push(ctor.name.clone());
                }
                insert(&mut schema, ctor)?;
            }
        }
    }
    resolve_bare(&mut schema)?;

    let mut roots = mtproto_names;
    for line in allow.lines().map(str::trim) {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        match schema.ctors.get(line) {
            Some(ctor) if ctor.function => roots.push(line.to_owned()),
            _ => {
                return Err(format!(
                    "allowlist entry `{line}` is not a function in the schema"
                ));
            }
        }
    }
    let (ctor_names, type_names) = closure(&schema, &roots)?;

    let mut types_bucket: BTreeMap<String, String> = BTreeMap::new();
    let mut enums_bucket: BTreeMap<String, String> = BTreeMap::new();
    let mut functions_bucket: BTreeMap<String, String> = BTreeMap::new();
    for name in &ctor_names {
        let ctor = &schema.ctors[name];
        let (ns, _) = split_ns(&ctor.name);
        if ctor.function {
            push(&mut functions_bucket, ns, &function_code(ctor));
        } else {
            push(&mut types_bucket, ns, &struct_code(ctor));
        }
    }
    for name in &type_names {
        let members: Vec<&Ctor> = schema.types[name]
            .iter()
            .map(|c| &schema.ctors[c])
            .collect();
        let (ns, _) = split_ns(name);
        push(&mut enums_bucket, ns, &enum_code(name, &members));
    }

    let mut out = String::from("// @generated by build.rs from tl/*.tl. Do not edit.\n");
    out.push_str(&module("types", &types_bucket));
    out.push_str(&module("enums", &enums_bucket));
    out.push_str(&module("functions", &functions_bucket));
    Ok(out)
}

fn push(bucket: &mut BTreeMap<String, String>, ns: &str, code: &str) {
    bucket.entry(ns.to_owned()).or_default().push_str(code);
}

const ALLOW: &str = "#[allow(unused_imports, dead_code, non_snake_case, non_camel_case_types, unused_variables, unused_mut, clippy::all)]";
const HEADER: &str = "use crate::tl::{Cursor, DeserializeError, Deserializable, Identifiable, RawObject, RemoteCall, Serializable};\n";

/// One `pub mod` holding the root items and one nested module per namespace.
fn module(name: &str, bucket: &BTreeMap<String, String>) -> String {
    let mut out = format!("{ALLOW}\npub mod {name} {{\n{HEADER}");
    if let Some(root) = bucket.get("") {
        out.push_str(root);
    }
    for (ns, body) in bucket {
        if ns.is_empty() {
            continue;
        }
        let _ = write!(out, "{ALLOW}\npub mod {ns} {{\n{HEADER}{body}}}\n");
    }
    out.push_str("}\n");
    out
}

fn insert(schema: &mut Schema, ctor: Ctor) -> Result<(), String> {
    if schema.ctors.contains_key(&ctor.name) {
        return Err(format!("duplicate definition of `{}`", ctor.name));
    }
    if !ctor.function {
        match &ctor.result {
            Ty::Named(ty) => schema
                .types
                .entry(ty.clone())
                .or_default()
                .push(ctor.name.clone()),
            other => {
                return Err(format!(
                    "constructor `{}` has result {other:?}, expected a boxed type",
                    ctor.name
                ));
            }
        }
    }
    schema.ctors.insert(ctor.name.clone(), ctor);
    Ok(())
}

/// Replace every `%T` with the bare constructor of `T`, which must be unique.
fn resolve_bare(schema: &mut Schema) -> Result<(), String> {
    let types = schema.types.clone();
    let resolve = |ty: &mut Ty| -> Result<(), String> {
        fn walk(ty: &mut Ty, types: &BTreeMap<String, Vec<String>>) -> Result<(), String> {
            match ty {
                Ty::BareType(name) => {
                    let members = types
                        .get(name.as_str())
                        .ok_or_else(|| format!("bare type of unknown type `{name}`"))?;
                    match members.as_slice() {
                        [only] => *ty = Ty::Bare(only.clone()),
                        _ => {
                            return Err(format!(
                                "bare type `%{name}` needs exactly one constructor"
                            ));
                        }
                    }
                }
                Ty::Vector(inner) | Ty::BareVector(inner) => walk(inner, types)?,
                Ty::Opt { inner, .. } => walk(inner, types)?,
                _ => {}
            }
            Ok(())
        }
        walk(ty, &types)
    };
    for ctor in schema.ctors.values_mut() {
        for param in &mut ctor.params {
            resolve(&mut param.ty)?;
        }
        resolve(&mut ctor.result)?;
    }
    Ok(())
}

/// The constructors and boxed types reachable from `roots`.
fn closure(
    schema: &Schema,
    roots: &[String],
) -> Result<(BTreeSet<String>, BTreeSet<String>), String> {
    let mut ctors = BTreeSet::new();
    let mut types = BTreeSet::new();
    let mut work: Vec<String> = roots.to_vec();
    while let Some(name) = work.pop() {
        if !ctors.insert(name.clone()) {
            continue;
        }
        let ctor = schema
            .ctors
            .get(&name)
            .ok_or_else(|| format!("unknown constructor `{name}`"))?;
        let mut tys: Vec<&Ty> = ctor.params.iter().map(|p| &p.ty).collect();
        if ctor.function {
            tys.push(&ctor.result);
        }
        for ty in tys {
            visit(ty, schema, &mut types, &mut work);
        }
    }
    Ok((ctors, types))
}

fn visit(ty: &Ty, schema: &Schema, types: &mut BTreeSet<String>, work: &mut Vec<String>) {
    match ty {
        Ty::Named(name) => {
            if types.insert(name.clone()) {
                work.extend(schema.types.get(name).into_iter().flatten().cloned());
            }
        }
        Ty::Bare(name) => work.push(name.clone()),
        Ty::Vector(inner) | Ty::BareVector(inner) => visit(inner, schema, types, work),
        Ty::Opt { inner, .. } => visit(inner, schema, types, work),
        _ => {}
    }
}

/// Parse one definition line. Returns `None` for built-in and skipped constructors.
fn parse_definition(line: &str, function: bool) -> Result<Option<Ctor>, String> {
    let body = line
        .strip_suffix(';')
        .ok_or_else(|| format!("definition without ';': {line}"))?
        .trim();
    let (lhs, rhs) = body
        .rsplit_once(" = ")
        .ok_or_else(|| format!("definition without ' = ': {line}"))?;
    let mut tokens = lhs.split_whitespace();
    let head = tokens
        .next()
        .ok_or_else(|| format!("empty definition: {line}"))?;
    let (name, explicit_id) = match head.split_once('#') {
        Some((name, hex)) => {
            let id = u32::from_str_radix(hex, 16)
                .map_err(|_| format!("bad constructor id in `{head}`"))?;
            (name.to_owned(), Some(id))
        }
        None => (head.to_owned(), None),
    };
    if SKIPPED.contains(&name.as_str()) {
        return Ok(None);
    }
    let mut generics = Vec::new();
    let mut raw_params = Vec::new();
    for token in tokens {
        if let Some(inner) = token.strip_prefix('{').and_then(|t| t.strip_suffix('}')) {
            let (gname, _) = inner
                .split_once(':')
                .ok_or_else(|| format!("bad generic `{token}` in {line}"))?;
            generics.push(gname.to_owned());
        } else {
            raw_params.push(token);
        }
    }
    let mut params = Vec::new();
    for token in &raw_params {
        if *token == "#" {
            return Err(format!("unnamed flags parameter in {line}"));
        }
        let (pname, pty) = token
            .split_once(':')
            .ok_or_else(|| format!("bad parameter `{token}` in {line}"))?;
        params.push(Param {
            name: pname.to_owned(),
            ty: parse_ty(pty, &generics).map_err(|e| format!("{e} in {line}"))?,
        });
    }
    let result = parse_ty(rhs.trim(), &generics).map_err(|e| format!("{e} in {line}"))?;
    let id = match explicit_id {
        Some(id) => id,
        None => {
            // Normalised combinator text: no `{}` braces, one space between lexemes, no `;`.
            let mut normalised = name.clone();
            for token in lhs.split_whitespace().skip(1) {
                let token = token.replace(['{', '}'], "");
                if !token.is_empty() {
                    normalised.push(' ');
                    normalised.push_str(&token);
                }
            }
            normalised.push_str(" = ");
            normalised.push_str(rhs.trim());
            crc32(normalised.as_bytes())
        }
    };
    Ok(Some(Ctor {
        name,
        id,
        params,
        generics,
        result,
        function,
    }))
}

fn parse_ty(text: &str, generics: &[String]) -> Result<Ty, String> {
    let text = text.trim();
    if text == "#" {
        return Ok(Ty::Flags);
    }
    if let Some((cond, inner)) = text.split_once('?') {
        let (flags, bit) = cond
            .split_once('.')
            .ok_or_else(|| format!("bad flag condition `{text}`"))?;
        let bit: u32 = bit
            .parse()
            .map_err(|_| format!("bad flag bit in `{text}`"))?;
        if bit > 31 {
            return Err(format!("flag bit out of range in `{text}`"));
        }
        return Ok(Ty::Opt {
            flags: flags.to_owned(),
            bit,
            inner: Box::new(parse_ty(inner, generics)?),
        });
    }
    if let Some(name) = text.strip_prefix('!') {
        return Ok(Ty::Generic(name.to_owned()));
    }
    match text {
        "int" => return Ok(Ty::Int),
        "long" => return Ok(Ty::Long),
        "double" => return Ok(Ty::Double),
        "string" => return Ok(Ty::Str),
        "bytes" => return Ok(Ty::Bytes),
        "int128" => return Ok(Ty::Int128),
        "int256" => return Ok(Ty::Int256),
        "Bool" => return Ok(Ty::Bool),
        "true" | "True" => return Ok(Ty::TrueFlag),
        "Object" => return Ok(Ty::Object),
        _ => {}
    }
    for (prefix, boxed) in [("Vector<", true), ("vector<", false)] {
        if let Some(inner) = text.strip_prefix(prefix).and_then(|t| t.strip_suffix('>')) {
            let inner = Box::new(parse_ty(inner, generics)?);
            return Ok(if boxed {
                Ty::Vector(inner)
            } else {
                Ty::BareVector(inner)
            });
        }
    }
    if let Some(name) = text.strip_prefix('%') {
        let name = name
            .strip_prefix('(')
            .and_then(|t| t.strip_suffix(')'))
            .unwrap_or(name);
        return Ok(Ty::BareType(name.to_owned()));
    }
    if generics.iter().any(|g| g == text) {
        return Ok(Ty::Generic(text.to_owned()));
    }
    // The case of the base name decides: `help.Config` is a type, `help.config` a constructor.
    match split_ns(text).1.chars().next() {
        Some(c) if c.is_ascii_uppercase() => Ok(Ty::Named(text.to_owned())),
        Some(c) if c.is_ascii_lowercase() => Ok(Ty::Bare(text.to_owned())),
        _ => Err(format!("unrecognised type `{text}`")),
    }
}

/// CRC32 (IEEE, reflected), table-driven.
fn crc32(data: &[u8]) -> u32 {
    let mut table = [0u32; 256];
    for (i, slot) in table.iter_mut().enumerate() {
        let mut c = i as u32;
        for _ in 0..8 {
            c = if c & 1 != 0 {
                0xEDB8_8320 ^ (c >> 1)
            } else {
                c >> 1
            };
        }
        *slot = c;
    }
    let mut crc = !0u32;
    for &byte in data {
        crc = table[((crc ^ u32::from(byte)) & 0xFF) as usize] ^ (crc >> 8);
    }
    !crc
}

fn split_ns(name: &str) -> (&str, &str) {
    match name.rsplit_once('.') {
        Some((ns, base)) => (ns, base),
        None => ("", name),
    }
}

/// `getConfig` becomes `GetConfig`, `server_DH_params_ok` becomes `ServerDHParamsOk`.
fn camel(base: &str) -> String {
    base.split('_')
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                Some(first) => first.to_ascii_uppercase().to_string() + chars.as_str(),
                None => String::new(),
            }
        })
        .collect()
}

fn safe(name: &str) -> String {
    if matches!(name, "self" | "super" | "crate" | "Self") {
        format!("{name}_")
    } else if KEYWORDS.contains(&name) {
        format!("r#{name}")
    } else {
        name.to_owned()
    }
}

fn ctor_path(full: &str) -> String {
    let (ns, base) = split_ns(full);
    if ns.is_empty() {
        format!("crate::tl::generated::types::{}", camel(base))
    } else {
        format!("crate::tl::generated::types::{ns}::{}", camel(base))
    }
}

fn type_path(full: &str) -> String {
    let (ns, base) = split_ns(full);
    if ns.is_empty() {
        format!("crate::tl::generated::enums::{}", camel(base))
    } else {
        format!("crate::tl::generated::enums::{ns}::{}", camel(base))
    }
}

fn gen_decl(generics: &[String]) -> String {
    if generics.is_empty() {
        String::new()
    } else {
        format!("<{}>", generics.join(", "))
    }
}

fn gen_bounds(generics: &[String]) -> String {
    if generics.is_empty() {
        String::new()
    } else {
        let bounds: Vec<String> = generics
            .iter()
            .map(|g| format!("{g}: crate::tl::RemoteCall"))
            .collect();
        format!("<{}>", bounds.join(", "))
    }
}

/// The Rust type of a value of `ty`; `boxed` wraps direct struct and enum references in `Box`
/// so that recursive schemas stay finite.
fn rust_ty(ty: &Ty, boxed: bool) -> String {
    let wrap = |path: String| {
        if boxed { format!("Box<{path}>") } else { path }
    };
    match ty {
        Ty::Int => "i32".to_owned(),
        Ty::Long => "i64".to_owned(),
        Ty::Double => "f64".to_owned(),
        Ty::Int128 => "[u8; 16]".to_owned(),
        Ty::Int256 => "[u8; 32]".to_owned(),
        Ty::Bytes => "Vec<u8>".to_owned(),
        Ty::Str => "String".to_owned(),
        Ty::Bool | Ty::TrueFlag => "bool".to_owned(),
        Ty::Object => "RawObject".to_owned(),
        Ty::Flags => "u32".to_owned(),
        Ty::Vector(inner) | Ty::BareVector(inner) => format!("Vec<{}>", rust_ty(inner, false)),
        Ty::Named(name) => wrap(type_path(name)),
        Ty::Bare(name) => wrap(ctor_path(name)),
        Ty::BareType(name) => format!("/* unresolved %{name} */"),
        Ty::Generic(name) => name.clone(),
        Ty::Opt { inner, .. } => match inner.as_ref() {
            Ty::TrueFlag => "bool".to_owned(),
            other => format!("Option<{}>", rust_ty(other, true)),
        },
    }
}

fn field_ty(ty: &Ty) -> String {
    rust_ty(ty, true)
}

/// Expression reading a value of `ty` from `buf`. `sized` names the already-read length for an
/// `Object` that follows a `bytes:int` length field.
fn de_expr(ty: &Ty, boxed: bool, sized: Option<&str>) -> String {
    let expr = match ty {
        Ty::Int => "<i32 as Deserializable>::deserialize(buf)?".to_owned(),
        Ty::Long => "<i64 as Deserializable>::deserialize(buf)?".to_owned(),
        Ty::Double => "<f64 as Deserializable>::deserialize(buf)?".to_owned(),
        Ty::Int128 => "<[u8; 16] as Deserializable>::deserialize(buf)?".to_owned(),
        Ty::Int256 => "<[u8; 32] as Deserializable>::deserialize(buf)?".to_owned(),
        Ty::Bool => "<bool as Deserializable>::deserialize(buf)?".to_owned(),
        Ty::Flags => "<u32 as Deserializable>::deserialize(buf)?".to_owned(),
        Ty::Bytes => "crate::tl::read_bytes(buf)?".to_owned(),
        Ty::Str => "crate::tl::read_string(buf)?".to_owned(),
        Ty::Object => match sized {
            Some(len) => format!("RawObject::read_sized(buf, {len})?"),
            None => "<RawObject as Deserializable>::deserialize(buf)?".to_owned(),
        },
        Ty::Vector(inner) => format!(
            "crate::tl::read_vector(buf, |buf| Ok({}))?",
            de_expr(inner, false, None)
        ),
        Ty::BareVector(inner) => format!(
            "crate::tl::read_bare_vector(buf, |buf| Ok({}))?",
            de_expr(inner, false, None)
        ),
        Ty::Named(name) => format!("<{} as Deserializable>::deserialize(buf)?", type_path(name)),
        Ty::Bare(name) => format!("{}::read_body(buf)?", ctor_path(name)),
        Ty::BareType(_) | Ty::Generic(_) | Ty::TrueFlag | Ty::Opt { .. } => {
            "compile_error!(\"type without a deserialisation rule\")".to_owned()
        }
    };
    match ty {
        Ty::Named(_) | Ty::Bare(_) if boxed => format!("Box::new({expr})"),
        _ => expr,
    }
}

/// Statement serialising `value` (an expression of reference type) of `ty`.
fn ser_stmt(ty: &Ty, value: &str, depth: usize) -> String {
    match ty {
        Ty::Bytes => format!("crate::tl::write_bytes(buf, {value});"),
        Ty::Str => format!("crate::tl::write_bytes(buf, ({value}).as_bytes());"),
        Ty::Vector(inner) => {
            let item = format!("i{depth}");
            format!(
                "crate::tl::write_vector_header(buf, ({value}).len()); for {item} in ({value}).iter() {{ {} }}",
                ser_stmt(inner, &item, depth + 1)
            )
        }
        Ty::BareVector(inner) => {
            let item = format!("i{depth}");
            format!(
                "crate::tl::write_len(buf, ({value}).len()); for {item} in ({value}).iter() {{ {} }}",
                ser_stmt(inner, &item, depth + 1)
            )
        }
        Ty::Opt { inner, .. } => match inner.as_ref() {
            Ty::TrueFlag => String::new(),
            other => {
                let item = format!("x{depth}");
                format!(
                    "if let Some({item}) = {value} {{ {} }}",
                    ser_stmt(other, &item, depth + 1)
                )
            }
        },
        Ty::TrueFlag | Ty::Flags => String::new(),
        _ => format!("({value}).serialize(buf);"),
    }
}

fn struct_fields(ctor: &Ctor) -> Vec<&Param> {
    ctor.params
        .iter()
        .filter(|p| !matches!(p.ty, Ty::Flags))
        .collect()
}

fn serialize_body(ctor: &Ctor) -> String {
    let mut s = String::from("buf.extend_from_slice(&Self::CONSTRUCTOR_ID.to_le_bytes());\n");
    for flags in ctor.params.iter().filter(|p| matches!(p.ty, Ty::Flags)) {
        let _ = writeln!(s, "let mut flags_{}: u32 = 0;", flags.name);
        for p in &ctor.params {
            if let Ty::Opt {
                flags: f,
                bit,
                inner,
            } = &p.ty
            {
                if *f != flags.name {
                    continue;
                }
                let cond = match inner.as_ref() {
                    Ty::TrueFlag => format!("self.{}", safe(&p.name)),
                    _ => format!("self.{}.is_some()", safe(&p.name)),
                };
                let _ = writeln!(s, "if {cond} {{ flags_{} |= 1u32 << {bit}; }}", flags.name);
            }
        }
    }
    for p in &ctor.params {
        match &p.ty {
            Ty::Flags => {
                let _ = writeln!(s, "buf.extend_from_slice(&flags_{}.to_le_bytes());", p.name);
            }
            Ty::Opt { inner, .. } if matches!(inner.as_ref(), Ty::TrueFlag) => {}
            Ty::Opt { inner, .. } => {
                let stmt = ser_stmt(inner, "x", 0);
                let _ = writeln!(s, "if let Some(x) = &self.{} {{ {stmt} }}", safe(&p.name));
            }
            ty => {
                let _ = writeln!(
                    s,
                    "{}",
                    ser_stmt(ty, &format!("&self.{}", safe(&p.name)), 0)
                );
            }
        }
    }
    s
}

fn read_body(ctor: &Ctor) -> String {
    let mut s = String::from("buf.enter()?;\n");
    let mut bytes_len: Option<String> = None;
    for p in &ctor.params {
        let name = &p.name;
        match &p.ty {
            Ty::Flags => {
                let _ = writeln!(
                    s,
                    "let flags_{name}: u32 = <u32 as Deserializable>::deserialize(buf)?;"
                );
            }
            Ty::Opt { flags, bit, inner } => {
                let cond = format!("flags_{flags} & (1u32 << {bit}) != 0");
                match inner.as_ref() {
                    Ty::TrueFlag => {
                        let _ = writeln!(s, "let f_{name}: bool = {cond};");
                    }
                    other => {
                        let _ = writeln!(
                            s,
                            "let f_{name} = if {cond} {{ Some({}) }} else {{ None }};",
                            de_expr(other, true, None)
                        );
                    }
                }
            }
            ty => {
                let sized = if matches!(ty, Ty::Object) {
                    bytes_len.clone()
                } else {
                    None
                };
                let _ = writeln!(s, "let f_{name} = {};", de_expr(ty, true, sized.as_deref()));
            }
        }
        if name == "bytes" && p.ty == Ty::Int {
            bytes_len = Some("f_bytes".to_owned());
        }
    }
    s.push_str("buf.leave();\n");
    s.push_str("Ok(Self {\n");
    for p in struct_fields(ctor) {
        let _ = writeln!(s, "{}: f_{},", safe(&p.name), p.name);
    }
    s.push_str("})\n");
    s
}

fn struct_code(ctor: &Ctor) -> String {
    let (_, base) = split_ns(&ctor.name);
    let name = camel(base);
    let mut s = String::new();
    let _ = writeln!(s, "#[derive(Clone, Debug, PartialEq)]");
    let _ = writeln!(s, "pub struct {name} {{");
    for p in struct_fields(ctor) {
        let _ = writeln!(s, "    pub {}: {},", safe(&p.name), field_ty(&p.ty));
    }
    let _ = writeln!(s, "}}");
    let _ = writeln!(
        s,
        "impl Identifiable for {name} {{ const CONSTRUCTOR_ID: u32 = 0x{:08x}; }}",
        ctor.id
    );
    let _ = writeln!(
        s,
        "impl Serializable for {name} {{ fn serialize(&self, buf: &mut Vec<u8>) {{\n{}}} }}",
        serialize_body(ctor)
    );
    let _ = writeln!(
        s,
        "impl {name} {{ pub(crate) fn read_body(buf: &mut Cursor<'_>) -> Result<Self, DeserializeError> {{\n{}}} }}",
        read_body(ctor)
    );
    let _ = writeln!(
        s,
        "impl Deserializable for {name} {{ fn deserialize(buf: &mut Cursor<'_>) -> Result<Self, DeserializeError> {{ buf.expect_constructor(0x{:08x})?; Self::read_body(buf) }} }}",
        ctor.id
    );
    s
}

fn function_code(ctor: &Ctor) -> String {
    let (_, base) = split_ns(&ctor.name);
    let name = camel(base);
    let decl = gen_decl(&ctor.generics);
    let bounds = gen_bounds(&ctor.generics);
    let mut s = String::new();
    let _ = writeln!(s, "#[derive(Clone, Debug, PartialEq)]");
    let _ = writeln!(s, "pub struct {name}{decl} {{");
    for p in struct_fields(ctor) {
        let _ = writeln!(s, "    pub {}: {},", safe(&p.name), field_ty(&p.ty));
    }
    let _ = writeln!(s, "}}");
    let _ = writeln!(
        s,
        "impl{bounds} Identifiable for {name}{decl} {{ const CONSTRUCTOR_ID: u32 = 0x{:08x}; }}",
        ctor.id
    );
    let _ = writeln!(
        s,
        "impl{bounds} Serializable for {name}{decl} {{ fn serialize(&self, buf: &mut Vec<u8>) {{\n{}}} }}",
        serialize_body(ctor)
    );
    let ret = match &ctor.result {
        Ty::Generic(g) => format!("<{g} as RemoteCall>::Return"),
        other => rust_ty(other, false),
    };
    let _ = writeln!(
        s,
        "impl{bounds} RemoteCall for {name}{decl} {{ type Return = {ret}; }}"
    );
    s
}

fn enum_code(type_name: &str, members: &[&Ctor]) -> String {
    let (_, base) = split_ns(type_name);
    let name = camel(base);
    let mut s = String::new();
    let _ = writeln!(s, "#[derive(Clone, Debug, PartialEq)]");
    let _ = writeln!(s, "pub enum {name} {{");
    for ctor in members {
        let (_, cbase) = split_ns(&ctor.name);
        let _ = writeln!(s, "    {}({}),", camel(cbase), ctor_path(&ctor.name));
    }
    let _ = writeln!(s, "}}");
    let _ = writeln!(
        s,
        "impl Serializable for {name} {{ fn serialize(&self, buf: &mut Vec<u8>) {{ match self {{"
    );
    for ctor in members {
        let (_, cbase) = split_ns(&ctor.name);
        let _ = writeln!(s, "Self::{}(x) => x.serialize(buf),", camel(cbase));
    }
    let _ = writeln!(s, "}} }} }}");
    let _ = writeln!(
        s,
        "impl Deserializable for {name} {{ fn deserialize(buf: &mut Cursor<'_>) -> Result<Self, DeserializeError> {{ let id = buf.peek_u32()?; match id {{"
    );
    for ctor in members {
        let (_, cbase) = split_ns(&ctor.name);
        let _ = writeln!(
            s,
            "0x{:08x} => Ok(Self::{}(<{} as Deserializable>::deserialize(buf)?)),",
            ctor.id,
            camel(cbase),
            ctor_path(&ctor.name)
        );
    }
    let _ = writeln!(
        s,
        "_ => Err(DeserializeError::UnknownConstructor(id)), }} }} }}"
    );
    s
}

/// CRC32 of the test string from the TL specification, pinned by the generator's unit test.
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc32_matches_standard_check_value() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
    }

    #[test]
    fn vector_constructor_id_from_the_spec() {
        let line = "vector t:Type # [ t ] = Vector t";
        assert_eq!(crc32(line.as_bytes()), 0x1cb5_c415);
    }
}
