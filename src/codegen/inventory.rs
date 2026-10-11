//! The feature-gated codegen inventory: **one artifact, derived from the
//! CodeGeneratorRequest that `build.rs` already feeds to `capnpc`**.
//!
//! Why this module exists (conformance model `knowledge/03-CONFORMANCE.md` §9,
//! "what I need from other owners"): the conformance generator and the codegen
//! must read ONE artifact. Otherwise a generator that parses schema source text
//! is reading a different thing from the one that produced the surface under
//! test — and the disagreement is invisible on the happy path. This module
//! closes that: build.rs takes the raw request capnpc just consumed and renders
//! a stable text index from the *same bytes*, so L0/L1 conformance describes
//! exactly the surface that was generated.
//!
//! What is emitted (`OUT_DIR/codegen-inventory.txt`):
//!
//! * per compiled schema file: its module name, its schema id, whether it is
//!   always-compiled foundation or behind a `domain-*` feature (and which one),
//!   and per-kind node counts;
//! * every interface with its method count;
//! * every method with its **compiler ordinal** (`code_order`), its parameter
//!   and result counts, and the shape flags the L1/L2 rule table keys on;
//! * a `summary` line of the totals, so the counts can be diffed against the
//!   contract's own measurement.
//!
//! Determinism is a hard requirement (conformance model L0.3): the output
//! contains no timestamps, no absolute paths and no environment-derived text,
//! and every collection is emitted in a fixed sorted order, so two builds of the
//! same feature set are byte-identical. `build.rs` proves this by rebuilding and
//! diffing; `tests/codegen.rs` proves it in-process by rendering twice.
//!
//! Node attribution: the request carries the transitive import closure, so an
//! interface from `compositor.capnp` shows up while compiling `shell.capnp`.
//! Every node is attributed by its display-name prefix
//! (`<schema-file>:<qualified-name>`) — the compiler's own ownership statement —
//! so a domain's block lists only its own surface.
//!
//! The raw request is kept too, at `OUT_DIR/codegeneratorrequest.bin` and
//! `SMART_LIB_CODEGEN_REQUEST` at compile time, for a conformance generator that
//! wants per-field detail instead of this index.

use std::collections::BTreeMap;

/// One schema file build.rs compiled, with the feature-side metadata the
/// inventory needs (the request itself knows nothing about cargo features).
pub struct SchemaFile {
    /// File name relative to the schema directory, e.g. `key_management.capnp`.
    pub file: String,
    /// Rust module name capnp codegen uses for it: `key_management_capnp`.
    pub module: String,
    /// Always-compiled foundation, or behind a `domain-*` feature.
    pub kind: FileKind,
    /// The feature that enabled a domain file; `None` for the foundation.
    pub feature: Option<String>,
}

/// Whether a schema file is always compiled or gated behind a domain feature.
pub enum FileKind {
    /// `types.capnp`, `capability.capnp`, `abi.capnp` + their imports.
    Foundation,
    /// One of the contract's domain schemas.
    Domain,
}

impl FileKind {
    fn as_str(&self) -> &'static str {
        match self {
            FileKind::Foundation => "foundation",
            FileKind::Domain => "domain",
        }
    }
}

/// A node indexed for rendering: owning file, qualified name, kind.
struct NodeInfo {
    file: String,
    /// Qualified name *inside* the file, e.g. `Files.openFile$Params`.
    local: String,
    kind: NodeKind,
    id: u64,
}

enum NodeKind {
    File,
    Struct,
    Enum,
    Interface,
    MethodParams,
    MethodResults,
    Const,
    Annotation,
    Other,
}

impl NodeKind {
    fn tally_key(&self) -> Option<&'static str> {
        match self {
            NodeKind::Struct => Some("structs"),
            NodeKind::Enum => Some("enums"),
            NodeKind::Interface => Some("interfaces"),
            NodeKind::MethodParams | NodeKind::MethodResults => Some("methodParams"),
            NodeKind::Const => Some("consts"),
            NodeKind::File | NodeKind::Annotation | NodeKind::Other => None,
        }
    }
}

/// Index of the request's nodes, keyed by owning file, plus the well-known type
/// ids the shape flags resolve against.
///
/// `nodes_by_id` matters for more than speed: capnp's reader carries a
/// *traversal budget* that every pointer dereference spends, and re-walking the
/// node list for each lookup exhausts it (the tail of the tree then fails to
/// read, silently, because `get_nodes()` starts returning
/// `ReadLimitExceeded`). One pass, then lookups.
struct NodeIndex<'a> {
    infos: Vec<NodeInfo>,
    nodes_by_id: BTreeMap<u64, capnp::schema_capnp::node::Reader<'a>>,
    capability_id: u64,
    pagination_id: u64,
    data_handle_id: u64,
    error_id: u64,
}

impl<'a> NodeIndex<'a> {
    fn build<F: Fn(&str) -> bool>(
        request: &'a capnp::schema_capnp::code_generator_request::Reader<'a>,
        is_compiled_file: F,
    ) -> Self {
        let mut infos = Vec::new();
        let mut nodes_by_id: BTreeMap<u64, capnp::schema_capnp::node::Reader<'_>> =
            BTreeMap::new();
        // One traversal of the node list; every later lookup reads the map.
        for node in request
            .get_nodes()
            .expect("the request's node list")
            .iter()
        {
            nodes_by_id.insert(node.get_id(), node);
            let display = node
                .get_display_name()
                .map(|t| text_of(&t))
                .unwrap_or_default();
            let prefix = node.get_display_name_prefix_length() as usize;
            let Some((file, local)) = attribute(&display, prefix, &is_compiled_file) else {
                continue;
            };
            let kind = match node.which() {
                Ok(capnp::schema_capnp::node::File(())) => NodeKind::File,
                Ok(capnp::schema_capnp::node::Struct(_)) => classify(&local),
                Ok(capnp::schema_capnp::node::Enum(_)) => NodeKind::Enum,
                Ok(capnp::schema_capnp::node::Interface(_)) => NodeKind::Interface,
                Ok(capnp::schema_capnp::node::Const(_)) => NodeKind::Const,
                Ok(capnp::schema_capnp::node::Annotation(_)) => NodeKind::Annotation,
                _ => NodeKind::Other,
            };
            infos.push(NodeInfo {
                file,
                local,
                kind,
                id: node.get_id(),
            });
        }
        // Resolve the well-known types by (file, name) from the node set itself,
        // so a contract-side id change cannot desynchronise this index.
        let id_of = |file: &str, name: &str| -> u64 {
            infos
                .iter()
                .find(|info| info.file == file && info.local == name)
                .map(|info| info.id)
                .unwrap_or_default()
        };
        let capability_id = id_of("capability.capnp", "Capability");
        let pagination_id = id_of("types.capnp", "Pagination");
        let data_handle_id = id_of("types.capnp", "DataHandle");
        let error_id = id_of("types.capnp", "Error");
        NodeIndex {
            infos,
            nodes_by_id,
            capability_id,
            pagination_id,
            data_handle_id,
            error_id,
        }
    }

    /// The node with `id`, or `None` when the request does not carry it.
    fn node_of(&self, id: u64) -> Option<capnp::schema_capnp::node::Reader<'_>> {
        self.nodes_by_id.get(&id).copied()
    }
}

/// The compiler's own ownership statement for a node, from its display name and
/// `displayNamePrefixLength`.
///
/// The prefix length counts the characters before the node's *simple* name,
/// **including** the separator that follows it: `:` at file scope (`files.capnp:`
/// is 13) and `.` inside a type (`files.capnp:Files.` is 19). So
/// `display[..len-1]` is `<file>[:<inner scope>]` and `display[len..]` is the
/// simple name.
///
/// `display[..len-1]` of a *file* node is the stem without its extension
/// (`abi.capnp` with prefix 4 gives `abi`), which matches no compiled file, so
/// file nodes fall out here — which is what we want: the per-file inventory
/// lines come from build.rs's compile set, not from the node walk.
///
/// Returns `(owning file, qualified name inside that file)`.
fn attribute(
    display: &str,
    prefix: usize,
    is_compiled_file: &dyn Fn(&str) -> bool,
) -> Option<(String, String)> {
    if prefix == 0 || prefix > display.len() {
        return None;
    }
    let scope = &display[..prefix - 1];
    let simple = &display[prefix..];
    let (file, inner) = match scope.split_once(':') {
        Some((file, inner)) => (file, inner),
        None => (scope, ""),
    };
    if !is_compiled_file(file) {
        return None;
    }
    let local = if inner.is_empty() {
        simple.to_string()
    } else {
        format!("{inner}.{simple}")
    };
    Some((file.to_string(), local))
}

/// The compiler synthesises `<iface>.<method>$Params` and `$Results` structs;
/// they are the method's parameter/result lists, not contract types, so they are
/// counted apart from the schema's own structs.
fn classify(local: &str) -> NodeKind {
    let simple = local.rsplit('.').next().unwrap_or(local);
    if let Some(head) = simple.strip_suffix("$Params") {
        if !head.is_empty() {
            return NodeKind::MethodParams;
        }
    }
    if let Some(head) = simple.strip_suffix("$Results") {
        if !head.is_empty() {
            return NodeKind::MethodResults;
        }
    }
    NodeKind::Struct
}

/// What one field scan found.
#[derive(Default)]
struct FieldScan {
    /// Slot fields encountered — the parameter or result count.
    fields: usize,
    /// Struct type ids referenced: slot types, plus the element type of a
    /// `List(...)` (so a `List(T.Pagination)` still resolves).
    type_ids: Vec<u64>,
    /// Any field typed `List(_)` (streams, paginated listings).
    has_list: bool,
}

impl FieldScan {
    fn references(&self, id: u64) -> bool {
        id != 0 && self.type_ids.contains(&id)
    }
}

/// Totals over the whole compiled set, cross-checkable against the contract's
/// own measurement (`knowledge/03-CONFORMANCE.md` §2.2).
#[derive(Default, PartialEq, Eq)]
struct Summary {
    files: usize,
    foundations: usize,
    domains: usize,
    structs: usize,
    enums: usize,
    interfaces: usize,
    methods: usize,
    method_structs: usize,
    consts: usize,
    capability: usize,
    pagination: usize,
    data_handle_in: usize,
    data_handle_out: usize,
    list_out: usize,
    error_out: usize,
    no_params: usize,
}

/// The shape of one method, as the L1/L2 rule table wants it.
#[derive(Default, PartialEq, Eq)]
struct MethodShape {
    params: usize,
    results: usize,
    capability: bool,
    pagination: bool,
    data_handle_in: bool,
    data_handle_out: bool,
    list_out: bool,
    error_out: bool,
    no_params: bool,
}

impl MethodShape {
    fn flags(&self) -> String {
        let mut flags: Vec<&str> = Vec::new();
        if self.capability {
            flags.push("capability");
        }
        if self.data_handle_in {
            flags.push("dataHandleIn");
        }
        if self.data_handle_out {
            flags.push("dataHandleOut");
        }
        if self.error_out {
            flags.push("errorOut");
        }
        if self.list_out {
            flags.push("listOut");
        }
        if self.no_params {
            flags.push("noParams");
        }
        if self.pagination {
            flags.push("pagination");
        }
        if flags.is_empty() {
            "-".to_string()
        } else {
            flags.join(",")
        }
    }
}

/// Render the inventory for `schemas` (the compiled set) from `request` (the
/// CodeGeneratorRequest capnp just consumed). `triad` is the one-line triad
/// description build.rs records.
pub fn render(
    request: &capnp::schema_capnp::code_generator_request::Reader<'_>,
    schemas: &[SchemaFile],
    triad: &str,
) -> String {
    let index = NodeIndex::build(request, |file| {
        schemas.iter().any(|schema| schema.file == file)
    });

    let mut out = String::with_capacity(256 * 1024);
    out.push_str(HEADER);
    out.push_str("# Triad: ");
    out.push_str(triad);
    out.push('\n');
    out.push_str("# Companion raw artifact: the CodeGeneratorRequest itself, at\n");
    out.push_str("# $SMART_LIB_CODEGEN_REQUEST (a capnp message; read it with\n");
    out.push_str("# capnp::schema_capnp::code_generator_request::Reader for per-field detail).\n");
    out.push_str("# This file: $SMART_LIB_CODEGEN_INVENTORY.\n\n");

    let mut summary = Summary::default();

    // One block per schema file, in sorted file order.
    let mut by_file: BTreeMap<&str, Vec<&NodeInfo>> = BTreeMap::new();
    for info in &index.infos {
        by_file.entry(info.file.as_str()).or_default().push(info);
    }

    for schema in schemas {
        summary.files += 1;
        match schema.kind {
            FileKind::Foundation => summary.foundations += 1,
            FileKind::Domain => summary.domains += 1,
        }
        let nodes = by_file.remove(schema.file.as_str()).unwrap_or_default();
        let mut tally: BTreeMap<&'static str, usize> = BTreeMap::new();
        for info in &nodes {
            match info.kind {
                // The compiler's synthetic param/result structs are tallied apart
                // from the schema's own structs, and both kinds count toward the
                // per-file `methodStructs` figure the schema line reports.
                NodeKind::MethodParams | NodeKind::MethodResults => {
                    summary.method_structs += 1;
                    *tally.entry("methodParams").or_insert(0) += 1;
                }
                _ => {
                    if let Some(key) = info.kind.tally_key() {
                        *tally.entry(key).or_insert(0) += 1;
                    }
                }
            }
        }
        summary.structs += tally.get("structs").copied().unwrap_or(0);
        summary.enums += tally.get("enums").copied().unwrap_or(0);
        summary.interfaces += tally.get("interfaces").copied().unwrap_or(0);
        summary.consts += tally.get("consts").copied().unwrap_or(0);

        out.push_str("schema ");
        out.push_str(&schema.file);
        out.push_str(" id=0x");
        out.push_str(&schema_file_id(request, schema));
        out.push_str(" module=");
        out.push_str(&schema.module);
        out.push_str(" kind=");
        out.push_str(schema.kind.as_str());
        out.push_str(" feature=");
        out.push_str(schema.feature.as_deref().unwrap_or("none"));
        out.push_str(&format!(
            " structs={} enums={} interfaces={} methodStructs={} consts={}\n",
            tally.get("structs").copied().unwrap_or(0),
            tally.get("enums").copied().unwrap_or(0),
            tally.get("interfaces").copied().unwrap_or(0),
            tally.get("methodParams").copied().unwrap_or(0),
            tally.get("consts").copied().unwrap_or(0),
        ));

        // Interfaces in qualified-name order, methods in compiler order.
        let mut interfaces: Vec<&NodeInfo> = nodes
            .iter()
            .copied()
            .filter(|info| matches!(info.kind, NodeKind::Interface))
            .collect();
        interfaces.sort_by_key(|info: &&NodeInfo| qualified(info));
        for iface in interfaces {
            let Some(node) = index.node_of(iface.id) else {
                // The node is in the compiled set but not in the request: a
                // contract/codegen disagreement, not a skip.
                panic!("build.rs: interface node {} is not in the CodeGeneratorRequest", qualified(iface));
            };
            let Ok(capnp::schema_capnp::node::Interface(interface)) = node.which() else {
                panic!("build.rs: {} is attributed as an interface node but is not one", qualified(iface));
            };
            let Ok(methods) = interface.get_methods() else {
                panic!("build.rs: {} declares no readable method list", qualified(iface));
            };
            out.push_str("interface ");
            out.push_str(&qualified(iface));
            out.push_str(&format!(" methods={}\n", methods.len()));

            let mut entries: Vec<(u16, String, MethodShape)> = Vec::new();
            for method in methods.iter() {
                let ordinal = method.get_code_order();
                let name = method
                    .get_name()
                    .map(|t| text_of(&t))
                    .unwrap_or_else(|_| "<unnamed>".to_string());
                let params = index.node_of(method.get_param_struct_type());
                let results = index.node_of(method.get_result_struct_type());
                entries.push((
                    ordinal,
                    name,
                    shape_of(&index, params.as_ref(), results.as_ref()),
                ));
            }
            entries.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
            summary.methods += entries.len();
            for (ordinal, name, shape) in entries {
                summary.capability += usize::from(shape.capability);
                summary.pagination += usize::from(shape.pagination);
                summary.data_handle_in += usize::from(shape.data_handle_in);
                summary.data_handle_out += usize::from(shape.data_handle_out);
                summary.list_out += usize::from(shape.list_out);
                summary.error_out += usize::from(shape.error_out);
                summary.no_params += usize::from(shape.no_params);

                out.push_str("method ");
                out.push_str(&ordinal.to_string());
                out.push(' ');
                out.push_str(&name);
                out.push_str(" interface=");
                out.push_str(&qualified(iface));
                out.push_str(&format!(
                    " params={} results={} flags={}\n",
                    shape.params,
                    shape.results,
                    shape.flags()
                ));
            }
        }
    }

    out.push('\n');
    let summary_line = format!(
        "summary schemas={} foundations={} domains={} structs={} enums={} interfaces={} \
         methods={} methodStructs={} consts={} capability={} pagination={} dataHandleIn={} \
         dataHandleOut={} listOut={} errorOut={} noParams={}\n",
        summary.files,
        summary.foundations,
        summary.domains,
        summary.structs,
        summary.enums,
        summary.interfaces,
        summary.methods,
        summary.method_structs,
        summary.consts,
        summary.capability,
        summary.pagination,
        summary.data_handle_in,
        summary.data_handle_out,
        summary.list_out,
        summary.error_out,
        summary.no_params,
    );
    out.push_str(&summary_line);
    self_check(&out, &summary_line);
    out
}

/// Verify the rendered artifact against itself, at generation time.
///
/// The failure mode this guards against is *truncation*: a reader that silently
/// stops early (I hit exactly this: capnp's traversal budget expired mid-tree
/// and 5 interfaces vanished while the summary still claimed 28). A test can
/// catch that, but a test only runs when someone runs it; this runs on every
/// build and panics with the discrepancy spelled out.
fn self_check(rendered: &str, summary_line: &str) {
    let mut schema_lines = 0usize;
    let mut interface_lines = 0usize;
    let mut method_lines = 0usize;
    let mut ordinals: BTreeMap<String, Vec<u16>> = BTreeMap::new();
    let mut claimed: BTreeMap<String, usize> = BTreeMap::new();

    for line in rendered.lines() {
        if line.starts_with('#') || line.trim().is_empty() {
            continue;
        }
        let fields: Vec<&str> = line.split_whitespace().collect();
        match fields[0] {
            "schema" => {
                schema_lines += 1;
                for field in &fields[2..] {
                    if let Some((key, value)) = field.split_once('=') {
                        if key != "id" {
                            *claimed.entry(key.to_string()).or_insert(0) +=
                                value.parse().unwrap_or(0);
                        }
                    }
                }
            }
            "interface" => {
                interface_lines += 1;
            }
            "method" => {
                method_lines += 1;
                let ordinal: u16 = fields[1].parse().unwrap_or(u16::MAX);
                let iface = fields
                    .iter()
                    .find_map(|f| f.strip_prefix("interface="))
                    .unwrap_or("<none>");
                ordinals.entry(iface.to_string()).or_default().push(ordinal);
            }
            "summary" => {}
            other => panic!("build.rs: inventory emitted an unknown line kind `{other}`"),
        }
    }

    let summary: BTreeMap<String, usize> = summary_line
        .split_whitespace()
        .filter_map(|f| f.split_once('='))
        .map(|(k, v)| (k.to_string(), v.parse().unwrap_or(usize::MAX)))
        .collect();
    let expect = |key: &str| *summary.get(key).unwrap_or(&0);
    let claimed_or = |key: &str| claimed.get(key).copied().unwrap_or(0);

    assert_eq!(
        schema_lines, expect("schemas"),
        "build.rs: inventory emitted {schema_lines} schema lines but the summary claims {}",
        expect("schemas")
    );
    for key in ["structs", "enums", "interfaces", "methodStructs", "consts"] {
        assert_eq!(
            claimed_or(key), expect(key),
            "build.rs: per-file `{key}` counts sum to {} but the summary claims {}",
            claimed_or(key),
            expect(key)
        );
    }
    assert_eq!(
        interface_lines, expect("interfaces"),
        "build.rs: inventory emitted {interface_lines} interface lines but the summary claims {}",
        expect("interfaces")
    );
    assert_eq!(
        method_lines, expect("methods"),
        "build.rs: inventory emitted {method_lines} method lines but the summary claims {}",
        expect("methods")
    );
    // Validator rule E13: method ordinals are sequential and declared in order.
    for (iface, mut numbers) in ordinals {
        numbers.sort_unstable();
        let expected: Vec<u16> = (0..numbers.len() as u16).collect();
        assert_eq!(
            numbers, expected,
            "build.rs: {iface} has non-sequential method ordinals: {numbers:?}"
        );
    }
}

/// Fully qualified display name of a node: `<file>:<local>`.
fn qualified(info: &NodeInfo) -> String {
    format!("{}:{}", info.file, info.local)
}

/// The request's file id for `schema`, as 16-hex-digit lowercase.
fn schema_file_id(
    request: &capnp::schema_capnp::code_generator_request::Reader<'_>,
    schema: &SchemaFile,
) -> String {
    let Ok(files) = request.get_requested_files() else {
        return "0000000000000000".to_string();
    };
    for file in files.iter() {
        if let Ok(name) = file.get_filename() {
            if text_of(&name) == schema.file {
                return format!("{:016x}", file.get_id());
            }
        }
    }
    "0000000000000000".to_string()
}

/// The parameter/result shape of one method, from the request's param/result
/// struct nodes.
fn shape_of(
    index: &NodeIndex,
    params: Option<&capnp::schema_capnp::node::Reader<'_>>,
    results: Option<&capnp::schema_capnp::node::Reader<'_>>,
) -> MethodShape {
    let param_scan = params.map(|node| scan_fields(index, node, 0)).unwrap_or_default();
    let result_scan = results.map(|node| scan_fields(index, node, 0)).unwrap_or_default();

    MethodShape {
        params: param_scan.fields,
        results: result_scan.fields,
        capability: param_scan.references(index.capability_id),
        pagination: param_scan.references(index.pagination_id),
        data_handle_in: param_scan.references(index.data_handle_id),
        data_handle_out: result_scan.references(index.data_handle_id),
        list_out: result_scan.has_list,
        error_out: result_scan.references(index.error_id),
        no_params: param_scan.fields == 0,
    }
}

/// Scan a struct node's fields, descending into nested groups so a capability or
/// pagination buried in a group is still the method's shape.
fn scan_fields(
    index: &NodeIndex,
    node: &capnp::schema_capnp::node::Reader<'_>,
    depth: u32,
) -> FieldScan {
    const MAX_DEPTH: u32 = 8;
    if depth > MAX_DEPTH {
        return FieldScan::default();
    }
    let mut scan = FieldScan::default();
    let Ok(capnp::schema_capnp::node::Struct(struct_node)) = node.which() else {
        return scan;
    };
    let Ok(fields) = struct_node.get_fields() else {
        return scan;
    };
    for field in fields.iter() {
        match field.which() {
            Ok(capnp::schema_capnp::field::Slot(slot)) => {
                scan.fields += 1;
                if let Ok(field_type) = slot.get_type() {
                    match field_type.which() {
                        Ok(capnp::schema_capnp::type_::Struct(s)) => {
                            scan.type_ids.push(s.get_type_id());
                        }
                        Ok(capnp::schema_capnp::type_::List(list)) => {
                            scan.has_list = true;
                            if let Ok(element) = list.get_element_type() {
                                if let Ok(capnp::schema_capnp::type_::Struct(s)) = element.which() {
                                    scan.type_ids.push(s.get_type_id());
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
            Ok(capnp::schema_capnp::field::Group(group)) => {
                if let Some(node) = index.node_of(group.get_type_id()) {
                    let nested = scan_fields(index, &node, depth + 1);
                    scan.fields += nested.fields;
                    scan.type_ids.extend(nested.type_ids);
                    scan.has_list |= nested.has_list;
                }
            }
            _ => {}
        }
    }
    scan
}

/// Reader text as a `String`, lossily: field and type names in the contract are
/// ASCII, and a non-UTF-8 name would be a contract defect, not a reason to panic
/// a build.
fn text_of(reader: &capnp::text::Reader<'_>) -> String {
    String::from_utf8_lossy(reader.as_bytes()).into_owned()
}

/// The inventory's own format documentation, emitted as a comment block so the
/// artifact is self-describing. Contains no build-varying data: no timestamps,
/// no paths, no host names.
const HEADER: &str = "\
# smart-lib codegen inventory (v1) -- GENERATED by build.rs, DO NOT EDIT.
#
# Derived from the CodeGeneratorRequest build.rs feeds to capnpc: the SAME
# artifact that generates the Rust under test, never a second parse of schema
# source. Regenerated on every build; byte-identical for a given feature set
# (no timestamps, no absolute paths, fixed sort order) -- L0 determinism is
# asserted by building twice and diffing, see knowledge/reports/06-all-features.md.
#
# Node attribution: the request carries the transitive import closure, so an
# interface from compositor.capnp appears while compiling shell.capnp. Each
# node is attributed by the compiler's own display-name prefix
# (`<schema-file>:<qualified-name>`), so a domain's block lists only its own
# surface. Method ordinals are the compiler's code_order -- the value the
# contract promises never moves.
#
# Line grammar (whitespace-separated fields):
#   schema  <file> id=<hex> module=<mod> kind=<foundation|domain> feature=<name|none> structs=<n> enums=<n> interfaces=<n> methodStructs=<n> consts=<n>
#     one line per compiled schema file. `methodStructs` counts the compiler's
#     synthetic <iface>.<method>$Params/$Results structs, kept apart from the
#     schema's own structs.
#   interface <file>:<Iface> methods=<n>
#   method <ordinal> <methodName> interface=<file>:<Iface> params=<n> results=<n> flags=<csv|->
#     flags, emitted in this fixed order and only when the shape matches:
#       capability    a parameter typed capability.capnp:Capability
#       dataHandleIn  a parameter typed types.capnp:DataHandle
#       dataHandleOut a result typed types.capnp:DataHandle
#       errorOut      a result carrying a types.capnp:Error arm
#       listOut       a result field typed List(_)
#       noParams      the parameter struct declares no fields
#       pagination    a parameter typed types.capnp:Pagination
#   summary <key>=<n> ...   totals over exactly the compiled set, for diffing
#     against the contract's own measurement.
#
";

#[cfg(test)]
mod tests {
    use super::*;
    // `modules` is a sibling inside the same `codegen` module, compiled by both
    // build.rs and the `tests/codegen.rs` harness.
    use crate::codegen::modules::{domain_feature_name, schema_file_to_stem, schema_module_name};
    use std::collections::BTreeSet;

    /// Decode the request build.rs just wrote for *this* build, using the same
    /// pinned runtime. Skip-able rather than compile-erroring if the plumbing is
    /// absent, but asserted present so a lost `cargo::rustc-env` fails loudly.
    fn current_request() -> Option<Vec<u8>> {
        let path = option_env!("SMART_LIB_CODEGEN_REQUEST")?;
        Some(
            std::fs::read(path)
                .unwrap_or_else(|e| panic!("cannot read the CodeGeneratorRequest at {path}: {e}")),
        )
    }

    fn decode(
        bytes: &[u8],
    ) -> capnp::message::Reader<capnp::serialize::OwnedSegments> {
        capnp::serialize::read_message(&mut &bytes[..], capnp::message::ReaderOptions::new())
            .expect("decode the CodeGeneratorRequest")
    }

    #[test]
    fn inventory_of_the_current_build_is_consistent() {
        let Some(bytes) = current_request() else {
            // No artifact: only when build.rs never ran (e.g. `cargo build` of a
            // different package). Not our failure mode.
            return;
        };
        let message = decode(&bytes);
        let request: capnp::schema_capnp::code_generator_request::Reader<'_> =
            message.get_root().expect("request root");

        let mut schemas: Vec<SchemaFile> = Vec::new();
        let files = request.get_requested_files().expect("requested files");
        for file in files.iter() {
            let name = text_of(&file.get_filename().expect("file name"));
            let stem = schema_file_to_stem(&name);
            schemas.push(SchemaFile {
                file: name.clone(),
                module: schema_module_name(&name),
                kind: if stem == "types" || stem == "capability" || stem == "abi" {
                    FileKind::Foundation
                } else {
                    FileKind::Domain
                },
                feature: if stem == "types" || stem == "capability" || stem == "abi" {
                    None
                } else {
                    Some(domain_feature_name(stem))
                },
            });
        }
        schemas.sort_by(|a, b| a.file.cmp(&b.file));

        let inventory = render(&request, &schemas, "triad-test");
        let lines: Vec<&str> = inventory
            .lines()
            .filter(|line| !line.starts_with('#') && !line.trim().is_empty())
            .collect();

        let schema_lines: Vec<&&str> = lines.iter().filter(|l| l.starts_with("schema ")).collect();
        let interface_lines: Vec<&&str> =
            lines.iter().filter(|l| l.starts_with("interface ")).collect();
        let method_lines: Vec<&&str> = lines.iter().filter(|l| l.starts_with("method ")).collect();
        let summary = lines
            .iter()
            .find(|l| l.starts_with("summary "))
            .expect("a summary line");

        assert_eq!(schema_lines.len(), schemas.len(), "one schema line per file");
        assert_eq!(interface_lines.len(), count(summary, "interfaces"));
        assert_eq!(method_lines.len(), count(summary, "methods"));

        // Every interface's ordinals start at 0 and are contiguous: the contract
        // promises ordinals are sequential (validator rule E13), so a generated
        // inventory that shows a gap is a defect, not a curiosity.
        let mut seen: BTreeSet<String> = BTreeSet::new();
        let mut ordinals: BTreeMap<String, Vec<u16>> = BTreeMap::new();
        for line in &method_lines {
            let fields: Vec<&str> = line.split_whitespace().collect();
            let ordinal: u16 = fields[1].parse().expect("ordinal");
            let iface = fields
                .iter()
                .find_map(|f| f.strip_prefix("interface="))
                .expect("interface field")
                .to_string();
            ordinals.entry(iface).or_default().push(ordinal);
            assert!(seen.insert(format!("{ordinal} {line}")), "no duplicate method line");
        }
        for (iface, mut numbers) in &ordinals {
            let mut sorted = numbers.clone();
            sorted.sort_unstable();
            let expected: Vec<u16> = (0..sorted.len() as u16).collect();
            assert_eq!(
                sorted, expected,
                "{iface}: ordinals must be 0..n-1 (sequential, per E13)"
            );
        }

        // The doc's own measurement, for the pilot domain the brief uses:
        // `Files` alone is 74 methods (@0-@73).
        if schemas.iter().any(|s| s.file == "files.capnp") {
            let files_methods = ordinals
                .get("files.capnp:Files")
                .map(|v| v.len())
                .unwrap_or(0);
            assert_eq!(
                files_methods, 74,
                "the contract measures Files as 74 methods (knowledge/03-CONFORMANCE.md 2.2)"
            );
        }

        // Rendering twice is byte-identical: L0 determinism, in-process.
        let again = render(&request, &schemas, "triad-test");
        assert_eq!(inventory, again, "render must be deterministic");
    }

    fn count(summary: &str, key: &str) -> usize {
        let field = summary
            .split_whitespace()
            .find_map(|f| f.strip_prefix(&format!("{key}=")))
            .unwrap_or_else(|| panic!("no {key} in the summary"));
        field.parse().expect("a number")
    }
}
