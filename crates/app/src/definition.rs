//! ⌘click "go to definition" in the file viewer, without language servers.
//!
//! The symbol under the pointer is looked up the way a reader would:
//! 1. its import (TS/JS `import … from`, `require`; Rust `use`, also for
//!    qualified paths like `config::Config`), following the module to its
//!    file and, there, re-exports (`export … from`, `pub use`) a few levels;
//! 2. a declaration in the same file;
//! 3. a declaration anywhere in the project (grep), the closest file first.
//!
//! It is a heuristic: methods, overloads or macro-generated items may land
//! on the wrong declaration or none. Everything here runs off the UI thread.

use std::collections::HashMap;
use std::ops::Range;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, LazyLock};

use parking_lot::Mutex;
use regex::Regex;

/// Where a symbol is declared: file and 0-based line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Target {
    pub path: PathBuf,
    pub line: u32,
}

/// Re-export chains followed before giving up.
const MAX_DEPTH: usize = 4;
/// Files larger than this are not read while following imports.
const MAX_READ: u64 = 4 * 1024 * 1024;
/// Declarations collected by the project-wide fallback.
const MAX_PROJECT_HITS: usize = 50;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Lang {
    Ts,
    Rust,
    Other,
}

fn lang_of(path: &Path) -> Lang {
    match path.extension().and_then(|e| e.to_str()).unwrap_or("") {
        "ts" | "tsx" | "mts" | "cts" | "js" | "jsx" | "mjs" | "cjs" | "vue" | "svelte" => Lang::Ts,
        "rs" => Lang::Rust,
        _ => Lang::Other,
    }
}

fn is_ident(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '$'
}

/// Byte range of the identifier at `offset` (or ending right at it).
pub fn word_at(text: &str, offset: usize) -> Option<Range<usize>> {
    let offset = offset.min(text.len());
    if !text.is_char_boundary(offset) {
        return None;
    }
    let start = text[..offset]
        .char_indices()
        .rev()
        .take_while(|(_, c)| is_ident(*c))
        .last()
        .map_or(offset, |(i, _)| i);
    let end = offset
        + text[offset..]
            .char_indices()
            .take_while(|(_, c)| is_ident(*c))
            .last()
            .map_or(0, |(i, c)| i + c.len_utf8());
    let word = &text[start..end];
    (!word.is_empty() && !word.starts_with(|c: char| c.is_ascii_digit())).then_some(start..end)
}

fn line_of(text: &str, offset: usize) -> u32 {
    text[..offset.min(text.len())].bytes().filter(|&b| b == b'\n').count() as u32
}

/// Finds the declaration of the symbol at byte `offset` of `text` (the
/// content of `file`). `None` when there is nothing better to show, e.g. the
/// pointer already is on the declaration.
pub fn find(file: &Path, text: &str, offset: usize) -> Option<Target> {
    let range = word_at(text, offset)?;
    let sym = &text[range.clone()];
    let clicked_line = line_of(text, range.start);
    let lang = lang_of(file);

    let via_import = match lang {
        Lang::Ts => ts_via_import(file, text, range.clone()),
        Lang::Rust => rust_via_import(file, text, range.clone()),
        Lang::Other => None,
    };
    if via_import.is_some() {
        return via_import;
    }
    if let Some(line) = find_decl(text, sym, lang) {
        return (line != clicked_line).then(|| Target {
            path: file.to_path_buf(),
            line,
        });
    }
    find_in_project(file, sym, lang)
}

// ---- declarations ----------------------------------------------------------------

fn decl_regex(sym: &str, lang: Lang) -> Regex {
    let s = regex::escape(sym);
    let pattern = match lang {
        Lang::Ts => format!(
            r"(?m)^[ \t]*(?:export[ \t]+)?(?:default[ \t]+)?(?:declare[ \t]+)?(?:abstract[ \t]+)?(?:async[ \t]+)?(?:const[ \t]+enum|interface|type|class|enum|function\*?|const|let|var|namespace|module)[ \t]+{s}\b"
        ),
        Lang::Rust => format!(
            r#"(?m)^[ \t]*(?:pub(?:\([^)]*\))?[ \t]+)?(?:(?:async|unsafe|const|default|extern(?:[ \t]+"[^"]*")?)[ \t]+)*(?:fn|struct|enum|trait|type|mod|const|static|union)[ \t]+{s}\b|^[ \t]*macro_rules![ \t]*{s}\b"#
        ),
        Lang::Other => format!(
            r"(?m)^[ \t]*(?:(?:export|public|private|protected|internal|static|async|abstract|final|open|data|sealed|pub)[ \t]+)*(?:def|class|func|function|fun|interface|struct|enum|type|trait|fn|protocol|object)[ \t]+{s}\b"
        ),
    };
    Regex::new(&pattern).expect("declaration regex")
}

/// 0-based line of the first declaration of `sym` in `text`.
fn find_decl(text: &str, sym: &str, lang: Lang) -> Option<u32> {
    decl_regex(sym, lang).find(text).map(|m| line_of(text, m.start()))
}

fn read(path: &Path) -> Option<String> {
    let meta = std::fs::metadata(path).ok()?;
    if !meta.is_file() || meta.len() > MAX_READ {
        return None;
    }
    std::fs::read_to_string(path).ok()
}

/// Collapses `.` and `..` without touching the file system.
fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in path.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            c => out.push(c),
        }
    }
    out
}

// ---- TS / JS ------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
struct TsImport {
    module: String,
    /// Exported name, `default`, or `*` for a namespace import.
    name: String,
}

static TS_IMPORT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?s)\bimport\s+(?:type\s+)?([\w$*{}\s,]+?)\s+from\s*['"]([^'"]+)['"]"#)
        .expect("static regex is valid")
});
static TS_REQUIRE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?s)\b(?:const|let|var)\s+(\{[^}]*\}|[\w$]+)\s*=\s*require\(\s*['"]([^'"]+)['"]\s*\)"#)
        .expect("static regex is valid")
});
static TS_REEXPORT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?s)\bexport\s+(?:type\s+)?\{([^}]*)\}\s*(?:from\s*['"]([^'"]+)['"])?"#)
        .expect("static regex is valid")
});
static TS_EXPORT_STAR: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"\bexport\s+(?:type\s+)?\*\s+(?:as\s+([\w$]+)\s+)?from\s*['"]([^'"]+)['"]"#)
        .expect("static regex is valid")
});

/// `a, b as c, type d` → [(exported, local)].
fn ts_specifiers(list: &str) -> Vec<(String, String)> {
    list.split(',')
        .filter_map(|spec| {
            let spec = spec.trim().trim_start_matches("type ").trim();
            if spec.is_empty() {
                return None;
            }
            let mut parts = spec.split_whitespace();
            let name = parts.next()?.to_string();
            let local = match (parts.next(), parts.next()) {
                (Some("as"), Some(alias)) => alias.to_string(),
                _ => name.clone(),
            };
            Some((name, local))
        })
        .collect()
}

/// Local name → import, for every `import`/`require` in `text`.
fn ts_imports(text: &str) -> HashMap<String, TsImport> {
    let mut out = HashMap::new();
    let add_clause = |clause: &str, module: &str, out: &mut HashMap<String, TsImport>| {
        let clause = clause.trim();
        let (head, braces) = match clause.find('{') {
            Some(i) => (&clause[..i], Some(clause[i + 1..].trim_end_matches('}'))),
            None => (clause, None),
        };
        for part in head.split(',').map(str::trim).filter(|p| !p.is_empty()) {
            let import = if let Some(ns) = part.strip_prefix('*') {
                let local = ns.trim().trim_start_matches("as").trim();
                (local.to_string(), "*".to_string())
            } else {
                (part.to_string(), "default".to_string())
            };
            out.insert(
                import.0,
                TsImport {
                    module: module.to_string(),
                    name: import.1,
                },
            );
        }
        for (name, local) in braces.map(ts_specifiers).unwrap_or_default() {
            out.insert(
                local,
                TsImport {
                    module: module.to_string(),
                    name,
                },
            );
        }
    };
    for c in TS_IMPORT.captures_iter(text) {
        add_clause(&c[1], &c[2], &mut out);
    }
    for c in TS_REQUIRE.captures_iter(text) {
        let binding = c[1].trim();
        if binding.starts_with('{') {
            // `const { a, b: c } = require('m')`
            let list = binding.trim_matches(|ch| ch == '{' || ch == '}').replace(':', " as ");
            for (name, local) in ts_specifiers(&list) {
                out.insert(
                    local,
                    TsImport {
                        module: c[2].to_string(),
                        name,
                    },
                );
            }
        } else {
            out.insert(
                binding.to_string(),
                TsImport {
                    module: c[2].to_string(),
                    name: "*".to_string(),
                },
            );
        }
    }
    out
}

fn ts_via_import(file: &Path, text: &str, range: Range<usize>) -> Option<Target> {
    let sym = &text[range.clone()];
    let imports = ts_imports(text);
    // `ns.Symbol` with `import * as ns from 'm'`.
    let before = &text[..range.start];
    if let Some(obj) = before.strip_suffix('.') {
        let ns_start = obj
            .char_indices()
            .rev()
            .take_while(|(_, c)| is_ident(*c))
            .last()
            .map(|(i, _)| i)?;
        if let Some(imp) = imports.get(&obj[ns_start..]).filter(|i| i.name == "*") {
            let module = resolve_ts_module(file, &imp.module)?;
            return Some(ts_export(&module, sym, 0).unwrap_or(Target { path: module, line: 0 }));
        }
        return None; // a property of something else: not an import
    }
    let imp = imports.get(sym)?;
    let module = resolve_ts_module(file, &imp.module)?;
    if imp.name == "*" {
        return Some(Target { path: module, line: 0 });
    }
    Some(ts_export(&module, &imp.name, 0).unwrap_or(Target { path: module, line: 0 }))
}

/// Where module file `path` declares its export `name`, following re-exports.
fn ts_export(path: &Path, name: &str, depth: usize) -> Option<Target> {
    if depth > MAX_DEPTH {
        return None;
    }
    let text = read(path)?;
    let here = |line| {
        Some(Target {
            path: path.to_path_buf(),
            line,
        })
    };
    if name == "default" {
        static EXPORT_DEFAULT: LazyLock<Regex> =
            LazyLock::new(|| Regex::new(r"(?m)^[ \t]*export[ \t]+default\b").expect("static regex is valid"));
        if let Some(m) = EXPORT_DEFAULT.find(&text) {
            return here(line_of(&text, m.start()));
        }
        // `export { x as default }` falls through to the re-exports below.
    } else if let Some(line) = find_decl(&text, name, Lang::Ts) {
        return here(line);
    }
    for c in TS_REEXPORT.captures_iter(&text) {
        let Some((original, _)) = ts_specifiers(&c[1]).into_iter().find(|(_, exported)| exported == name) else {
            continue;
        };
        let line = line_of(&text, c.get_match().start());
        match c.get(2) {
            // `export { a as name } from 'm'`
            Some(module) => {
                let Some(next) = resolve_ts_module(path, module.as_str()) else {
                    return here(line);
                };
                return ts_export(&next, &original, depth + 1).or(Some(Target { path: next, line: 0 }));
            }
            // `export { local as name }`: the local is declared or imported here.
            None => {
                if let Some(l) = find_decl(&text, &original, Lang::Ts) {
                    return here(l);
                }
                if let Some(imp) = ts_imports(&text).get(&original) {
                    if let Some(next) = resolve_ts_module(path, &imp.module) {
                        return ts_export(&next, &imp.name, depth + 1).or(Some(Target { path: next, line: 0 }));
                    }
                }
                return here(line);
            }
        }
    }
    for c in TS_EXPORT_STAR.captures_iter(&text) {
        let Some(next) = resolve_ts_module(path, &c[2]) else {
            continue;
        };
        match c.get(1) {
            Some(ns) if ns.as_str() == name => return Some(Target { path: next, line: 0 }),
            Some(_) => {}
            None => {
                if let Some(t) = ts_export(&next, name, depth + 1) {
                    return Some(t);
                }
            }
        }
    }
    None
}

const TS_EXTS: &[&str] = &[
    "ts", "tsx", "d.ts", "mts", "cts", "js", "jsx", "mjs", "cjs", "vue", "svelte",
];

/// The file a module path stands for: as is, with an extension, or its index.
fn ts_file(base: &Path) -> Option<PathBuf> {
    if base.is_file() {
        // ESM TypeScript imports `./a.js` for `a.ts`: prefer the source.
        if matches!(
            base.extension().and_then(|e| e.to_str()),
            Some("js" | "mjs" | "cjs" | "jsx")
        ) {
            let stem = base.with_extension("");
            for ext in ["ts", "tsx", "mts", "cts", "d.ts"] {
                let p = PathBuf::from(format!("{}.{ext}", stem.display()));
                if p.is_file() {
                    return Some(p);
                }
            }
        }
        return Some(base.to_path_buf());
    }
    let with_ext = |p: &Path| {
        TS_EXTS
            .iter()
            .map(|ext| PathBuf::from(format!("{}.{ext}", p.display())))
            .find(|p| p.is_file())
    };
    with_ext(base)
        .or_else(|| {
            // `./a.js` written for `a.ts` that has no `.js` sibling.
            let ext = base.extension().and_then(|e| e.to_str())?;
            matches!(ext, "js" | "mjs" | "cjs" | "jsx")
                .then(|| with_ext(&base.with_extension("")))
                .flatten()
        })
        .or_else(|| with_ext(&base.join("index")))
}

fn resolve_ts_module(from: &Path, spec: &str) -> Option<PathBuf> {
    let dir = from.parent()?;
    if spec.starts_with("./") || spec.starts_with("../") || spec == "." || spec == ".." {
        return ts_file(&normalize(&dir.join(spec)));
    }
    if let Some(p) = tsconfig_resolve(dir, spec) {
        return Some(p);
    }
    node_module(dir, spec)
}

/// Strips `//` and `/* */` comments and trailing commas (tsconfig is JSONC).
fn jsonc(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    let mut in_str = false;
    while let Some(c) = chars.next() {
        if in_str {
            out.push(c);
            if c == '\\' {
                if let Some(n) = chars.next() {
                    out.push(n);
                }
            } else if c == '"' {
                in_str = false;
            }
            continue;
        }
        match (c, chars.peek()) {
            ('"', _) => {
                in_str = true;
                out.push(c);
            }
            ('/', Some('/')) => {
                for n in chars.by_ref() {
                    if n == '\n' {
                        out.push('\n');
                        break;
                    }
                }
            }
            ('/', Some('*')) => {
                chars.next();
                let mut prev = ' ';
                for n in chars.by_ref() {
                    if prev == '*' && n == '/' {
                        break;
                    }
                    prev = n;
                }
            }
            _ => out.push(c),
        }
    }
    static TRAILING_COMMA: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r",(\s*[}\]])").expect("static regex is valid"));
    TRAILING_COMMA.replace_all(&out, "$1").into_owned()
}

/// `compilerOptions` of a tsconfig, merged over the ones it `extends`.
fn compiler_options(config: &Path, depth: usize) -> Option<(PathBuf, serde_json::Map<String, serde_json::Value>)> {
    let json: serde_json::Value = serde_json::from_str(&jsonc(&read(config)?)).ok()?;
    let dir = config.parent()?.to_path_buf();
    let mut opts = serde_json::Map::new();
    let mut base_dir = dir.clone();
    if let Some(ext) = json
        .get("extends")
        .and_then(|e| e.as_str())
        .filter(|e| e.starts_with('.') && depth < MAX_DEPTH)
    {
        let mut parent = normalize(&dir.join(ext));
        if parent.extension().is_none() {
            parent.set_extension("json");
        }
        if let Some((d, o)) = compiler_options(&parent, depth + 1) {
            base_dir = d;
            opts = o;
        }
    }
    if let Some(own) = json.get("compilerOptions").and_then(|o| o.as_object()) {
        if own.contains_key("baseUrl") || own.contains_key("paths") {
            base_dir = dir;
        }
        opts.extend(own.clone());
    }
    Some((base_dir, opts))
}

fn tsconfig_resolve(dir: &Path, spec: &str) -> Option<PathBuf> {
    let config = dir
        .ancestors()
        .flat_map(|d| [d.join("tsconfig.json"), d.join("jsconfig.json")])
        .find(|p| p.is_file())?;
    let (config_dir, opts) = compiler_options(&config, 0)?;
    let base = opts
        .get("baseUrl")
        .and_then(|b| b.as_str())
        .map(|b| normalize(&config_dir.join(b)))
        .unwrap_or(config_dir);
    if let Some(paths) = opts.get("paths").and_then(|p| p.as_object()) {
        for (pattern, targets) in paths {
            let star = match pattern.split_once('*') {
                Some((pre, post))
                    if spec.starts_with(pre) && spec.ends_with(post) && spec.len() >= pre.len() + post.len() =>
                {
                    Some(&spec[pre.len()..spec.len() - post.len()])
                }
                None if pattern == spec => Some(""),
                _ => None,
            };
            let Some(star) = star else { continue };
            for t in targets.as_array().into_iter().flatten().filter_map(|t| t.as_str()) {
                if let Some(p) = ts_file(&normalize(&base.join(t.replace('*', star)))) {
                    return Some(p);
                }
            }
        }
    }
    opts.get("baseUrl").and_then(|_| ts_file(&base.join(spec)))
}

fn node_module(dir: &Path, spec: &str) -> Option<PathBuf> {
    let mut parts = spec.splitn(if spec.starts_with('@') { 3 } else { 2 }, '/');
    let pkg = if spec.starts_with('@') {
        format!("{}/{}", parts.next()?, parts.next()?)
    } else {
        parts.next()?.to_string()
    };
    let sub = parts.next();
    for d in dir.ancestors() {
        let nm = d.join("node_modules");
        if !nm.is_dir() {
            continue;
        }
        let types = nm.join("@types").join(pkg.trim_start_matches('@').replace('/', "__"));
        for root in [nm.join(&pkg), types] {
            if !root.is_dir() {
                continue;
            }
            if let Some(sub) = sub {
                if let Some(p) = ts_file(&root.join(sub)) {
                    return Some(p);
                }
                continue;
            }
            let manifest: Option<serde_json::Value> =
                read(&root.join("package.json")).and_then(|t| serde_json::from_str(&t).ok());
            for key in ["types", "typings", "module", "main"] {
                if let Some(p) = manifest
                    .as_ref()
                    .and_then(|m| m.get(key))
                    .and_then(|v| v.as_str())
                    .and_then(|e| ts_file(&normalize(&root.join(e))))
                {
                    return Some(p);
                }
            }
            if let Some(p) = ts_file(&root.join("index")) {
                return Some(p);
            }
        }
    }
    None
}

// ---- Rust -----------------------------------------------------------------------------

static RUST_USE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?m)^[ \t]*(?:pub(?:\([^)]*\))?[ \t]+)?use[ \t]+([^;]+);").expect("static regex is valid")
});

#[derive(Default, Debug)]
struct RustUses {
    /// Local name → full path.
    names: HashMap<String, Vec<String>>,
    /// Prefixes of `use a::b::*`.
    globs: Vec<Vec<String>>,
}

/// Splits on `sep` outside braces.
fn split_top(s: &str, sep: char) -> Vec<&str> {
    let (mut depth, mut start, mut out) = (0i32, 0, Vec::new());
    for (i, c) in s.char_indices() {
        match c {
            '{' => depth += 1,
            '}' => depth -= 1,
            c if c == sep && depth == 0 => {
                out.push(&s[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    out.push(&s[start..]);
    out
}

fn segments(path: &str) -> Vec<String> {
    path.split("::")
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

fn expand_use(prefix: &[String], tree: &str, uses: &mut RustUses) {
    let tree = tree.trim();
    if tree.ends_with('}') {
        if let Some(open) = tree.find('{') {
            let mut prefix = prefix.to_vec();
            prefix.extend(segments(&tree[..open]));
            for item in split_top(&tree[open + 1..tree.len() - 1], ',') {
                if !item.trim().is_empty() {
                    expand_use(&prefix, item, uses);
                }
            }
            return;
        }
    }
    let (path, alias) = match tree.split_once(" as ") {
        Some((p, a)) => (p, Some(a.trim())),
        None => (tree, None),
    };
    let mut full = prefix.to_vec();
    full.extend(segments(path));
    match full.last().map(String::as_str) {
        Some("*") => {
            full.pop();
            uses.globs.push(full);
        }
        Some("self") => {
            full.pop();
            if let Some(last) = full.last().cloned() {
                uses.names.insert(alias.unwrap_or(&last).to_string(), full);
            }
        }
        Some(last) => {
            let local = alias.unwrap_or(last).to_string();
            if local != "_" {
                uses.names.insert(local, full);
            }
        }
        None => {}
    }
}

fn rust_uses(text: &str) -> RustUses {
    let mut uses = RustUses::default();
    for c in RUST_USE.captures_iter(text) {
        // Line comments inside a multi-line `use` would break the tree.
        let tree: String = c[1]
            .lines()
            .map(|l| l.split("//").next().unwrap_or(""))
            .collect::<Vec<_>>()
            .join(" ");
        expand_use(&[], &tree, &mut uses);
    }
    uses
}

/// Nearest directory with a `Cargo.toml` that has a `[package]`.
fn crate_dir(file: &Path) -> Option<PathBuf> {
    file.ancestors()
        .skip(1)
        .find(|d| read(&d.join("Cargo.toml")).is_some_and(|t| t.contains("[package]")))
        .map(Path::to_path_buf)
}

fn crate_root(dir: &Path) -> Option<PathBuf> {
    [dir.join("src/lib.rs"), dir.join("src/main.rs")]
        .into_iter()
        .find(|p| p.is_file())
}

/// Directory holding the child modules of module file `file`.
fn children_dir(file: &Path) -> Option<PathBuf> {
    let name = file.file_name()?.to_str()?;
    if matches!(name, "mod.rs" | "lib.rs" | "main.rs") || file.parent().is_some_and(|p| p.ends_with("src/bin")) {
        file.parent().map(Path::to_path_buf)
    } else {
        Some(file.with_extension(""))
    }
}

fn child_module(file: &Path, name: &str) -> Option<PathBuf> {
    let dir = children_dir(file)?;
    // macOS file systems ignore case: `Config` must not find `config.rs`.
    let has = |dir: &Path, entry: &str| {
        std::fs::read_dir(dir)
            .into_iter()
            .flatten()
            .flatten()
            .any(|e| e.file_name() == entry)
    };
    let file = dir.join(format!("{name}.rs"));
    if has(&dir, &format!("{name}.rs")) && file.is_file() {
        return Some(file);
    }
    let mod_rs = dir.join(name).join("mod.rs");
    (has(&dir, name) && mod_rs.is_file()).then_some(mod_rs)
}

fn parent_module(file: &Path) -> Option<PathBuf> {
    let name = file.file_name()?.to_str()?;
    let dir = file.parent()?;
    let dir = if name == "mod.rs" { dir.parent()? } else { dir };
    if matches!(name, "lib.rs" | "main.rs") {
        return None;
    }
    let parent_dir_name = dir.file_name()?.to_str()?;
    if dir.ends_with("src") {
        return crate_root(dir.parent()?);
    }
    let up = dir.parent()?;
    [up.join(format!("{parent_dir_name}.rs")), dir.join("mod.rs")]
        .into_iter()
        .find(|p| p.is_file())
}

/// Root file of the workspace crate named `name` (`-` and `_` alike).
fn workspace_crate(file: &Path, name: &str) -> Option<PathBuf> {
    let root = project_root(file);
    let wanted = name.replace('-', "_");
    static CRATE_NAME: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r#"(?m)^\s*name\s*=\s*"([^"]+)""#).expect("static regex is valid"));
    let name_re = &*CRATE_NAME;
    let mut dirs = vec![root.clone()];
    for depth in 0..2 {
        let mut next = Vec::new();
        for d in &dirs {
            for e in std::fs::read_dir(d).into_iter().flatten().flatten() {
                let p = e.path();
                let fname = e.file_name();
                if !p.is_dir()
                    || fname
                        .to_str()
                        .is_some_and(|n| n.starts_with('.') || n == "target" || n == "node_modules")
                {
                    continue;
                }
                if let Some(t) = read(&p.join("Cargo.toml")) {
                    if name_re.captures(&t).is_some_and(|c| c[1].replace('-', "_") == wanted) {
                        return crate_root(&p);
                    }
                }
                if depth == 0 {
                    next.push(p);
                }
            }
        }
        dirs = next;
    }
    None
}

/// Declaration of the item at Rust path `segs`, seen from `file`.
fn resolve_rust_path(file: &Path, segs: &[String], depth: usize) -> Option<Target> {
    if depth > MAX_DEPTH || segs.is_empty() {
        return None;
    }
    let (mut module, rest) = match segs[0].as_str() {
        "crate" => (crate_root(&crate_dir(file)?)?, &segs[1..]),
        "self" => (file.to_path_buf(), &segs[1..]),
        "super" => {
            let mut m = file.to_path_buf();
            let mut i = 0;
            while segs.get(i).is_some_and(|s| s == "super") {
                m = parent_module(&m)?;
                i += 1;
            }
            (m, &segs[i..])
        }
        "Self" => return None,
        // `None`: a crate outside the project.
        first => (
            child_module(file, first).or_else(|| workspace_crate(file, first))?,
            &segs[1..],
        ),
    };
    if rest.is_empty() {
        return Some(Target { path: module, line: 0 });
    }
    for (i, seg) in rest.iter().enumerate() {
        let last = i + 1 == rest.len();
        let text = read(&module)?;
        // An item of this module (a type, or `Enum::Variant`'s enum).
        if let Some(line) = find_decl(&text, seg, Lang::Rust).filter(|_| !text_declares_mod(&text, seg)) {
            return Some(Target { path: module, line });
        }
        if let Some(child) = child_module(&module, seg) {
            if last {
                return Some(Target { path: child, line: 0 });
            }
            module = child;
            continue;
        }
        // Re-exported here: `pub use x::Seg;` or a glob.
        let uses = rust_uses(&text);
        if let Some(path) = uses.names.get(seg) {
            let mut full = path.clone();
            full.extend(rest[i + 1..].iter().cloned());
            return resolve_rust_path(&module, &full, depth + 1);
        }
        for glob in &uses.globs {
            let mut full = glob.clone();
            full.extend(rest[i..].iter().cloned());
            if let Some(t) = resolve_rust_path(&module, &full, depth + 1) {
                return Some(t);
            }
        }
        return None;
    }
    None
}

fn text_declares_mod(text: &str, name: &str) -> bool {
    Regex::new(&format!(
        r"(?m)^[ \t]*(?:pub(?:\([^)]*\))?[ \t]+)?mod[ \t]+{}[ \t]*;",
        regex::escape(name)
    ))
    .expect("an escaped name yields a valid regex")
    .is_match(text)
}

/// `a::b::` right before byte `start`, as segments.
fn rust_qualifier(text: &str, start: usize) -> Vec<String> {
    let mut segs = Vec::new();
    let mut end = start;
    while let Some(before) = text[..end].strip_suffix("::") {
        let s = before
            .char_indices()
            .rev()
            .take_while(|(_, c)| is_ident(*c))
            .last()
            .map(|(i, _)| i);
        let Some(s) = s.filter(|&s| s < before.len()) else {
            break;
        };
        segs.insert(0, before[s..].to_string());
        end = s;
    }
    segs
}

fn rust_via_import(file: &Path, text: &str, range: Range<usize>) -> Option<Target> {
    let sym = &text[range.clone()];
    let uses = rust_uses(text);
    let qual = rust_qualifier(text, range.start);
    let mut path = if let Some(first) = qual.first() {
        let mut p = uses.names.get(first).cloned().unwrap_or_else(|| vec![first.clone()]);
        p.extend(qual[1..].iter().cloned());
        p
    } else if let Some(p) = uses.names.get(sym) {
        return resolve_rust_path(file, p, 0);
    } else {
        // Not imported by name: declared here, or through a glob import.
        if find_decl(text, sym, Lang::Rust).is_some() {
            return None;
        }
        return uses.globs.iter().find_map(|g| {
            let mut p = g.clone();
            p.push(sym.to_string());
            resolve_rust_path(file, &p, 0)
        });
    };
    path.push(sym.to_string());
    resolve_rust_path(file, &path, 0)
}

// ---- file URIs (the editor's LSP hooks speak URIs) ----------------------------------

/// `file://` URI of `path`, percent-encoding all but unreserved characters.
pub fn file_uri(path: &Path) -> String {
    let mut out = String::from("file://");
    for b in path.to_string_lossy().bytes() {
        if b.is_ascii_alphanumeric() || b"-._~/".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// Inverse of [`file_uri`].
pub fn path_from_uri(uri: &str) -> Option<PathBuf> {
    let encoded = uri.strip_prefix("file://")?.as_bytes();
    let mut bytes = Vec::with_capacity(encoded.len());
    let mut i = 0;
    while i < encoded.len() {
        if encoded[i] == b'%' && i + 2 < encoded.len() {
            let hex = std::str::from_utf8(&encoded[i + 1..i + 3]).ok()?;
            bytes.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            bytes.push(encoded[i]);
            i += 1;
        }
    }
    String::from_utf8(bytes).ok().map(PathBuf::from)
}

// ---- project-wide fallback ------------------------------------------------------------

/// The repository root (`.git`), or the file's folder outside one.
pub fn project_root(file: &Path) -> PathBuf {
    let dir = file.parent().unwrap_or(file);
    dir.ancestors()
        .find(|d| d.join(".git").exists())
        .unwrap_or(dir)
        .to_path_buf()
}

fn find_in_project(file: &Path, sym: &str, lang: Lang) -> Option<Target> {
    let root = project_root(file);
    let pattern = decl_regex(sym, lang).as_str().trim_start_matches("(?m)").to_string();
    let query = fs_tools::content::ContentQuery {
        pattern,
        regex: true,
        case_sensitive: true,
        max_results: MAX_PROJECT_HITS,
    };
    let hits: Arc<Mutex<Vec<Target>>> = Arc::default();
    let sink = hits.clone();
    fs_tools::content::search(&root, &query, Arc::new(AtomicBool::new(false)), move |m| {
        let mut hits = sink.lock();
        hits.extend(m.lines.iter().map(|l| Target {
            path: m.path.clone(),
            line: l.line_number.saturating_sub(1) as u32,
        }));
    })
    .ok()?;
    let mut hits = std::mem::take(&mut *hits.lock());
    // Same language first, then the file closest to the one being read.
    let shared = |p: &Path| {
        p.components()
            .zip(file.components())
            .take_while(|(a, b)| a == b)
            .count()
    };
    hits.sort_by_key(|t| {
        (
            lang_of(&t.path) != lang,
            std::cmp::Reverse(shared(&t.path)),
            t.path.clone(),
            t.line,
        )
    });
    hits.into_iter().next()
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Tree(PathBuf);
    impl Tree {
        fn new(name: &str, files: &[(&str, &str)]) -> Self {
            let root = std::env::temp_dir().join(format!("gridterm-def-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&root);
            std::fs::create_dir_all(root.join(".git")).unwrap();
            for (path, text) in files {
                let p = root.join(path);
                std::fs::create_dir_all(p.parent().unwrap()).unwrap();
                std::fs::write(p, text).unwrap();
            }
            Tree(root)
        }
        fn at(&self, file: &str, needle: &str) -> Option<(String, u32)> {
            let path = self.0.join(file);
            let text = std::fs::read_to_string(&path).unwrap();
            let offset = text.find(needle).unwrap_or_else(|| panic!("{needle} not in {file}")) + 1;
            find(&path, &text, offset).map(|t| (t.path.strip_prefix(&self.0).unwrap().display().to_string(), t.line))
        }
    }
    impl Drop for Tree {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn hit(file: &str, line: u32) -> Option<(String, u32)> {
        Some((file.to_string(), line))
    }

    #[test]
    fn word_boundaries() {
        assert_eq!(word_at("let fooBar = 1", 6), Some(4..10));
        assert_eq!(word_at("let fooBar = 1", 10), Some(4..10));
        assert_eq!(word_at("a + 12", 5), None);
        assert_eq!(word_at("x = ñandú;", 6), Some(4..11));
    }

    #[test]
    fn ts_named_default_namespace_and_aliases() {
        let t = Tree::new("ts", &[
            ("src/app.ts", "import { User, type Role as R } from './models/user';\nimport Api from \"../lib/api.js\";\nimport * as util from './util';\nconst u: User = null as any;\nconst r: R = 'a';\nApi.get(); util.slugify('x');\n"),
            ("src/models/user.ts", "// users\n\nexport interface User {\n  id: string;\n}\nexport type Role = 'a' | 'b';\n"),
            ("lib/api.ts", "class Api {}\nexport default Api;\n"),
            ("src/util/index.ts", "export function slugify(s: string) { return s; }\n"),
        ]);
        assert_eq!(t.at("src/app.ts", "User = null"), hit("src/models/user.ts", 2));
        assert_eq!(t.at("src/app.ts", "R = 'a'"), hit("src/models/user.ts", 5));
        assert_eq!(t.at("src/app.ts", "Api.get"), hit("lib/api.ts", 1));
        assert_eq!(t.at("src/app.ts", "slugify("), hit("src/util/index.ts", 0));
        assert_eq!(t.at("src/app.ts", "util.slugify"), hit("src/util/index.ts", 0));
    }

    #[test]
    fn ts_barrels_paths_and_node_modules() {
        let t = Tree::new("ts2", &[
            ("tsconfig.json", "{\n  // comment\n  \"compilerOptions\": { \"baseUrl\": \".\", \"paths\": { \"@/*\": [\"src/*\"] }, },\n}\n"),
            ("src/page.tsx", "import { Button, Theme } from '@/ui';\nimport { z } from 'zod';\n<Button />; z;\n"),
            ("src/ui/index.ts", "export * from './button';\nexport { Theme as Theme } from './theme';\n"),
            ("src/ui/button.tsx", "\nexport const Button = () => null;\n"),
            ("src/ui/theme.ts", "\n\nexport enum Theme { Dark }\n"),
            ("node_modules/zod/package.json", "{ \"types\": \"./lib/index.d.ts\" }"),
            ("node_modules/zod/lib/index.d.ts", "export declare const z: any;\n"),
        ]);
        assert_eq!(t.at("src/page.tsx", "Button />"), hit("src/ui/button.tsx", 1));
        assert_eq!(t.at("src/page.tsx", "Theme }"), hit("src/ui/theme.ts", 2));
        assert_eq!(t.at("src/page.tsx", "z;"), hit("node_modules/zod/lib/index.d.ts", 0));
    }

    #[test]
    fn local_declaration_and_project_fallback() {
        let t = Tree::new(
            "local",
            &[
                ("a.ts", "function helper() {}\n\nhelper();\nglobalThing();\n"),
                ("b/c.ts", "export function globalThing() {}\n"),
            ],
        );
        assert_eq!(t.at("a.ts", "helper();"), hit("a.ts", 0));
        // On the declaration itself there is nowhere to go.
        assert_eq!(t.at("a.ts", "helper() {}"), None);
        assert_eq!(t.at("a.ts", "globalThing"), hit("b/c.ts", 0));
    }

    #[test]
    fn rust_use_trees_and_paths() {
        let t = Tree::new("rs", &[
            ("Cargo.toml", "[workspace]\nmembers = [\"app\", \"crates/cfg\"]\n"),
            ("app/Cargo.toml", "[package]\nname = \"app\"\n"),
            ("app/src/main.rs", "mod panels;\nuse crate::panels::{git::{GitPanel, Mode as M}, search};\nuse cfg_lib::Config;\nuse cfg_lib::prelude::*;\nfn main() { let _: GitPanel; let _: M; search::run(); let _: Config; Theme::Dark; cfg_lib::load(); }\n"),
            ("app/src/panels/mod.rs", "pub mod git;\npub mod search;\n"),
            ("app/src/panels/git.rs", "\npub struct GitPanel;\npub enum Mode { A }\n"),
            ("app/src/panels/search.rs", "use super::git::GitPanel;\npub fn run() { let _: GitPanel; }\n"),
            ("crates/cfg/Cargo.toml", "[package]\nname = \"cfg-lib\"\n"),
            ("crates/cfg/src/lib.rs", "mod config;\npub mod prelude;\npub use config::Config;\npub fn load() {}\n"),
            ("crates/cfg/src/config.rs", "#[derive(Debug)]\npub struct Config;\n"),
            ("crates/cfg/src/prelude.rs", "pub enum Theme { Dark }\n"),
        ]);
        assert_eq!(
            t.at("app/src/main.rs", "GitPanel; let"),
            hit("app/src/panels/git.rs", 1)
        );
        assert_eq!(t.at("app/src/main.rs", "M; search"), hit("app/src/panels/git.rs", 2));
        assert_eq!(t.at("app/src/main.rs", "run()"), hit("app/src/panels/search.rs", 1));
        assert_eq!(
            t.at("app/src/main.rs", "Config; Theme"),
            hit("crates/cfg/src/config.rs", 1)
        );
        assert_eq!(
            t.at("app/src/main.rs", "Theme::Dark"),
            hit("crates/cfg/src/prelude.rs", 0)
        );
        assert_eq!(t.at("app/src/main.rs", "load()"), hit("crates/cfg/src/lib.rs", 3));
        assert_eq!(
            t.at("app/src/panels/search.rs", "GitPanel; }"),
            hit("app/src/panels/git.rs", 1)
        );
    }

    /// Against gridterm's own sources: real `use` trees and workspace crates.
    #[test]
    fn resolves_gridterm_itself() {
        let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let at = |file: &str, needle: &str| {
            let path = src.join(file);
            let text = std::fs::read_to_string(&path).unwrap();
            let t = find(&path, &text, text.find(needle).unwrap() + 1).expect(needle);
            let line = std::fs::read_to_string(&t.path)
                .unwrap()
                .lines()
                .nth(t.line as usize)
                .unwrap()
                .to_string();
            (t.path.file_name().unwrap().to_str().unwrap().to_string(), line)
        };
        let (file, line) = at("workspace/mod.rs", "SearchPanel, settings");
        assert_eq!(file, "search.rs");
        assert!(line.contains("pub struct SearchPanel"), "{line}");
        let (file, line) = at("panels/search.rs", "FileIndex, SearchHit");
        assert_eq!(file, "index.rs");
        assert!(line.contains("pub struct FileIndex"), "{line}");
    }

    #[test]
    fn uris_round_trip() {
        let p = Path::new("/tmp/a dir/ñ#1%.ts");
        let uri = file_uri(p);
        assert!(uri.parse::<lsp_types::Uri>().is_ok(), "{uri}");
        assert_eq!(path_from_uri(&uri).as_deref(), Some(p));
    }

    #[test]
    fn jsonc_is_parsed() {
        let v: serde_json::Value =
            serde_json::from_str(&jsonc("{ \"a\": \"http://x\", /* c */ \"b\": [1,], }")).unwrap();
        assert_eq!(v["a"], "http://x");
        assert_eq!(v["b"][0], 1);
    }
}
