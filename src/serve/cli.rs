//! The registry service: the command surface this host publishes, in the
//! contract's own `cli.capnp` vocabulary.
//!
//! # What the registry is for
//!
//! `cli.capnp` says it plainly: "The registry is the authority on what
//! commands exist and what each one needs. A client that guesses is a client
//! that lies." So this service publishes, per command:
//!
//! * the path the user types (`commandPath`);
//! * the schema identity the command routes through (`schemaId`,
//!   `interfaceName`, `methodName`);
//! * the presentation (`domain`, `summary`);
//! * the **method's identity** — the capnp ordinal the schema declares — and
//!   the **schema's node tree**, in the registration's open metadata.
//!
//! # Why the metadata carries what the contract does not declare
//!
//! `CommandRegistration` names the method but has no field for its ordinal,
//! and the contract has nowhere to publish a schema's node tree. A client
//! that never compiled the schema therefore cannot address the method. This
//! service publishes both through the contract-legal open metadata, under
//! stable keys:
//!
//! | key | meaning |
//! |---|---|
//! | `methodId` | the method's capnp ordinal, decimal text |
//! | `capnpSchema` | the schema's node tree (the contract's own `CodeGeneratorRequest`), hex |
//! | `reason` | why the command cannot run: the engine's own reason class and words |
//! | `degraded` | the named degraded capability, when the command answers but not completely |
//! | `composes` | the other contract methods one verb composes, comma separated |
//!
//! The schema's tree is identical for every command of that schema, so a
//! service publishes it ONCE per schema (on the first registration of that
//! schema); a client reads it from the registration it addresses, or from any
//! registration of the same schema.
//!
//! The founder's ruling on the schema itself (a declared `methodId` field and
//! a schema-tree reference) is reported, not assumed: the schema tree is not
//! edited here.

// The crate is `no_std` (law #1); this module is the std-gated seam.
extern crate std;

use alloc::boxed::Box;
use alloc::string::String;
use alloc::string::ToString;
use alloc::vec;
use alloc::vec::Vec;

use std::sync::Mutex;

use capnp::capability::FromClientHook;
use capnp::private::capability::ClientHook;

use crate::schema::cli_capnp::{
    cli_service, command_registration, AvailabilityReason,
};
use crate::schema::types_capnp;

/// The registry interface's id, from the contract's generated code.
pub const INTERFACE_ID: u64 = cli_service::_private::TYPE_ID;

/// The metadata key the **method's ordinal** is published under.
pub const METADATA_METHOD_ID: &str = "methodId";

/// The metadata key the **schema's node tree** is published under.
pub const METADATA_SCHEMA_TREE: &str = "capnpSchema";

/// One command this service publishes.
#[derive(Clone, Debug)]
pub struct PublishedCommand {
    /// The path the user types ("net status").
    pub path: String,
    /// The schema the command routes through (the canonical schema id).
    pub schema_id: u64,
    /// The interface the command calls, as the schema spells it.
    pub interface: String,
    /// The method the command calls, as the schema spells it.
    pub method: String,
    /// The presentation domain ("network").
    pub domain: String,
    /// The one-line summary.
    pub summary: String,
    /// The sort key within the domain (lower sorts first).
    pub order: u32,
    /// Whether a bare invocation (no arguments) is meaningful.
    pub no_args_valid: bool,
    /// Why the command cannot run right now, when it cannot: the contract's
    /// own `AvailabilityReason` and the domain's own reason.
    pub unavailable: Option<(AvailabilityReason, String)>,
    /// The named degraded capability, when the command answers but not
    /// completely.
    pub degraded: Option<String>,
    /// The registration's open metadata: the other contract methods one verb
    /// composes, and whatever else the domain publishes about it.
    pub metadata: Vec<(String, String)>,
}

/// The registry state: the commands this host publishes.
#[derive(Default)]
pub struct RegistryState {
    commands: Mutex<Vec<PublishedCommand>>,
}

impl RegistryState {
    /// An empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Publish a command.
    pub fn publish(&self, command: PublishedCommand) {
        if let Ok(mut guard) = self.commands.lock() {
            match guard.iter_mut().find(|entry| entry.path == command.path) {
                Some(entry) => *entry = command,
                None => guard.push(command),
            }
        }
    }

    /// Every published command, in publication order.
    pub fn commands(&self) -> Vec<PublishedCommand> {
        self.commands
            .lock()
            .map(|guard| guard.clone())
            .unwrap_or_default()
    }

    /// One published command, by path.
    pub fn command(&self, path: &str) -> Option<PublishedCommand> {
        self.commands
            .lock()
            .ok()
            .and_then(|guard| guard.iter().find(|entry| entry.path == path).cloned())
    }
}

/// The schema stem a schema id names, as the engine's own table records it.
///
/// The engine's generated table is keyed by (stem, interface); the registry
/// publishes the schema id, so the stem is found through the same table.
fn stem_for(schema_id: u64, interface: &str) -> Option<&'static str> {
    // The generated table has one arm per interface; the schema id is the
    // schema's own file id. The engine resolves it through the domain that
    // published the command, so the lookup here is over the same table the
    // codegen wrote.
    let stem = crate::contract::stem_for_schema_id(schema_id)?;
    // The interface must live in that schema: a command that names an
    // interface the schema does not declare has no ordinal, and the
    // registration says so by publishing none.
    crate::contract::interface_id(stem, interface)?;
    Some(stem)
}

/// Write an open metadata map (the contract's `T.StringMap`).
fn write_metadata(
    entries: &[(String, String)],
    builder: &mut crate::schema::types_capnp::string_map::Builder<'_>,
) {
    let mut map = builder.reborrow().init_entries(entries.len() as u32);
    for (index, (key, value)) in entries.iter().enumerate() {
        let mut entry = map.reborrow().get(index as u32);
        entry.set_key(key);
        entry.set_value(value);
    }
}

/// Hex-encode bytes (the schema's node tree travels as text).
fn hex_encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(char::from_digit(u32::from(byte >> 4), 16).unwrap_or('0'));
        out.push(char::from_digit(u32::from(byte & 0x0f), 16).unwrap_or('0'));
    }
    out
}

/// The registry server: the contract's own `CliService`.
pub struct CliServer {
    state: std::sync::Arc<RegistryState>,
    /// The schema trees this service publishes, by schema id: the schema's own
    /// node tree, so a client that never compiled the schema can address its
    /// methods.
    schema_trees: std::sync::Arc<Mutex<Vec<(u64, &'static [u8])>>>,
}

impl Clone for CliServer {
    fn clone(&self) -> Self {
        Self {
            state: self.state.clone(),
            schema_trees: self.schema_trees.clone(),
        }
    }
}

/// The registry's own state, plus what a registration needs.
struct RegistryView {
    commands: Vec<PublishedCommand>,
    schema_trees: Vec<(u64, &'static [u8])>,
}

impl CliServer {
    fn view(&self) -> RegistryView {
        RegistryView {
            commands: self.state.commands(),
            schema_trees: self
                .schema_trees
                .lock()
                .map(|guard| guard.clone())
                .unwrap_or_default(),
        }
    }
}

/// Write one registration into the contract's `CommandRegistration`.
fn write_registration(
    command: &PublishedCommand,
    tree: Option<&'static [u8]>,
    builder: &mut command_registration::Builder<'_>,
) {
    builder.set_command_path(&command.path);
    builder.reborrow().init_schema_id().set_id(command.schema_id);
    builder.set_interface_name(&command.interface);
    builder.set_method_name(&command.method);
    let (reason, detail) = match &command.unavailable {
        Some((reason, detail)) => (*reason, detail.clone()),
        None => (AvailabilityReason::Available, String::new()),
    };
    builder.set_unavailable_reason(reason);
    {
        let mut presentation = builder.reborrow().init_presentation();
        presentation.set_domain(&command.domain);
        presentation.set_summary(&command.summary);
        presentation.set_order(command.order);
        presentation.set_no_args_valid(command.no_args_valid);
    }
    // The metadata: the method's identity and the schema's node tree — the
    // two things the contract's registration does not carry — plus the
    // domain's own words about this command.
    let mut metadata = Vec::new();
    if let Some(stem) = stem_for(command.schema_id, &command.interface) {
        if let Some(ordinal) = crate::contract::method_id(stem, &command.interface, &command.method) {
            metadata.push((
                METADATA_METHOD_ID.to_string(),
                ordinal.to_string(),
            ));
        }
    }
    if let Some(tree) = tree {
        metadata.push((METADATA_SCHEMA_TREE.to_string(), hex_encode(tree)));
    }
    if !detail.is_empty() {
        metadata.push(("reason".to_string(), detail));
    }
    if let Some(degraded) = &command.degraded {
        metadata.push(("degraded".to_string(), degraded.clone()));
    }
    for (key, value) in &command.metadata {
        metadata.push((key.clone(), value.clone()));
    }
    write_metadata(&metadata, &mut builder.reborrow().init_metadata());
}

impl cli_service::Server for CliServer {
    /// Every registered command, with what it takes to run it.
    fn list_commands(
        self: std::rc::Rc<Self>,
        params: cli_service::ListCommandsParams,
        mut results: cli_service::ListCommandsResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        async move {
            let params = params.get()?;
            let page = params.get_pagination()?;
            let offset = page.get_offset() as usize;
            let limit = page.get_limit() as usize;
            let view = self.view();
            let selected: Vec<&PublishedCommand> = view
                .commands
                .iter()
                .filter(|command| command.unavailable.is_none())
                .skip(offset)
                .take(if limit == u32::MAX as usize {
                    usize::MAX
                } else {
                    limit
                })
                .collect();
            let mut builder = results.get();
            let mut list = builder.reborrow().init_commands(selected.len() as u32);
            for (index, command) in selected.iter().enumerate() {
                let tree = view
                    .schema_trees
                    .iter()
                    .find(|(id, _)| *id == command.schema_id)
                    .map(|(_, tree)| *tree);
                write_registration(
                    command,
                    tree,
                    &mut list.reborrow().get(index as u32),
                );
            }
            builder.set_total(
                view.commands
                    .iter()
                    .filter(|command| command.unavailable.is_none())
                    .count() as u64,
            );
            Ok(())
        }
    }

    /// The registered commands that cannot run right now, and why.
    fn list_unavailable(
        self: std::rc::Rc<Self>,
        _params: cli_service::ListUnavailableParams,
        mut results: cli_service::ListUnavailableResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        async move {
            let view = self.view();
            let selected: Vec<&PublishedCommand> = view
                .commands
                .iter()
                .filter(|command| command.unavailable.is_some())
                .collect();
            let mut builder = results.get();
            let mut list = builder.reborrow().init_commands(selected.len() as u32);
            for (index, command) in selected.iter().enumerate() {
                let tree = view
                    .schema_trees
                    .iter()
                    .find(|(id, _)| *id == command.schema_id)
                    .map(|(_, tree)| *tree);
                write_registration(
                    command,
                    tree,
                    &mut list.reborrow().get(index as u32),
                );
            }
            builder.set_total(selected.len() as u64);
            Ok(())
        }
    }

    /// One command's detail: the registration plus its description.
    fn get_command_info(
        self: std::rc::Rc<Self>,
        params: cli_service::GetCommandInfoParams,
        mut results: cli_service::GetCommandInfoResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        async move {
            let params = params.get()?;
            let path = crate::serve::text_of(params.get_command_path()?);
            let view = self.view();
            match view.commands.iter().find(|command| command.path == path) {
                Some(command) => {
                    let tree = view
                        .schema_trees
                        .iter()
                        .find(|(id, _)| *id == command.schema_id)
                        .map(|(_, tree)| *tree);
                    let mut info = results.get().init_result().init_info();
                    write_registration(command, tree, &mut info.reborrow().init_registration());
                    info.set_description(&command.summary);
                    Ok(())
                }
                None => {
                    let mut error = results.get().init_result().init_error();
                    error.set_code(types_capnp::ErrorCode::NotFound);
                    error.set_message(&alloc::format!(
                        "notFound: no command is registered at {path:?}"
                    ));
                    Ok(())
                }
            }
        }
    }

    /// The grammar a client parses user input against: the command paths this
    /// service publishes, one per line.
    fn get_syntax(
        self: std::rc::Rc<Self>,
        _params: cli_service::GetSyntaxParams,
        mut results: cli_service::GetSyntaxResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        async move {
            let view = self.view();
            let mut grammar = String::new();
            for command in &view.commands {
                grammar.push_str(&command.path);
                grammar.push('\n');
            }
            let mut syntax = results.get().init_syntax();
            syntax.set_grammar(&grammar);
            syntax.reborrow().init_version().set_major(0);
            syntax.reborrow().init_version().set_minor(1);
            // The domains the grammar covers, with their command counts — and
            // how many of them cannot run right now, so a client that knows
            // a domain is missing can say so.
            let mut domains: Vec<(String, usize, usize)> = Vec::new();
            for command in &view.commands {
                let unavailable = usize::from(command.unavailable.is_some());
                match domains.iter_mut().find(|(id, _, _)| *id == command.domain) {
                    Some(entry) => {
                        entry.1 += 1;
                        entry.2 += unavailable;
                    }
                    None => domains.push((command.domain.clone(), 1, unavailable)),
                }
            }
            let mut list = syntax.reborrow().init_domains(domains.len() as u32);
            for (index, (id, count, unavailable)) in domains.iter().enumerate() {
                let mut entry = list.reborrow().get(index as u32);
                entry.set_id(id);
                entry.set_label(id);
                entry.set_description("the commands this service publishes for it");
                entry.set_command_count(*count as u32);
                entry.set_unavailable_count(*unavailable as u32);
            }
            Ok(())
        }
    }

    /// This service serves domain contracts, not a shell: it does not execute
    /// commands. The refusal names the truth.
    fn execute(
        self: std::rc::Rc<Self>,
        _params: cli_service::ExecuteParams,
        _results: cli_service::ExecuteResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(Err(super::errors::decline(
            crate::core::ErrorCode::NotSupported,
            "notSupported: no-backend: this service publishes the command \
             registry; it does not execute commands. Call the domain method \
             the registry names.",
        )))
    }

    fn spawn_background(
        self: std::rc::Rc<Self>,
        _params: cli_service::SpawnBackgroundParams,
        _results: cli_service::SpawnBackgroundResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(Err(super::errors::decline(
            crate::core::ErrorCode::NotSupported,
            "notSupported: no-backend: this service publishes the command \
             registry; it does not run background jobs",
        )))
    }

    fn kill_job(
        self: std::rc::Rc<Self>,
        _params: cli_service::KillJobParams,
        _results: cli_service::KillJobResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(Err(super::errors::decline(
            crate::core::ErrorCode::NotSupported,
            "notSupported: no-backend: this service publishes the command \
             registry; it does not run jobs",
        )))
    }

    fn subscribe_job(
        self: std::rc::Rc<Self>,
        _params: cli_service::SubscribeJobParams,
        _results: cli_service::SubscribeJobResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(Err(super::errors::decline(
            crate::core::ErrorCode::NotSupported,
            "notSupported: no-backend: this service publishes the command \
             registry; it does not run jobs",
        )))
    }

    fn get_history(
        self: std::rc::Rc<Self>,
        _params: cli_service::GetHistoryParams,
        _results: cli_service::GetHistoryResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(Err(super::errors::decline(
            crate::core::ErrorCode::NotSupported,
            "notSupported: no-backend: this service publishes the command \
             registry; it keeps no command history",
        )))
    }

    fn clear_history(
        self: std::rc::Rc<Self>,
        _params: cli_service::ClearHistoryParams,
        _results: cli_service::ClearHistoryResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(Err(super::errors::decline(
            crate::core::ErrorCode::NotSupported,
            "notSupported: no-backend: this service publishes the command \
             registry; it keeps no command history",
        )))
    }

    fn get_completions(
        self: std::rc::Rc<Self>,
        params: cli_service::GetCompletionsParams,
        mut results: cli_service::GetCompletionsResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        async move {
            let params = params.get()?;
            let partial = crate::serve::text_of(params.get_partial()?);
            let view = self.view();
            let mut completions: Vec<String> = Vec::new();
            for command in &view.commands {
                if command.path.starts_with(&partial) && !completions.contains(&command.path) {
                    completions.push(command.path.clone());
                }
            }
            let mut list = results.get().init_completions(completions.len() as u32);
            for (index, completion) in completions.iter().enumerate() {
                list.reborrow().set(index as u32, completion.as_str());
            }
            Ok(())
        }
    }

    fn execute_streaming(
        self: std::rc::Rc<Self>,
        _params: cli_service::ExecuteStreamingParams,
        _results: cli_service::ExecuteStreamingResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(Err(super::errors::decline(
            crate::core::ErrorCode::NotSupported,
            "notSupported: no-backend: this service publishes the command \
             registry; it does not execute commands",
        )))
    }

    fn list_aliases(
        self: std::rc::Rc<Self>,
        _params: cli_service::ListAliasesParams,
        mut results: cli_service::ListAliasesResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        async move {
            // This service publishes no aliases: the paths it publishes ARE
            // the paths.
            results.get().init_aliases(0);
            Ok(())
        }
    }

    fn execute_pipeline(
        self: std::rc::Rc<Self>,
        _params: cli_service::ExecutePipelineParams,
        _results: cli_service::ExecutePipelineResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(Err(super::errors::decline(
            crate::core::ErrorCode::NotSupported,
            "notSupported: no-backend: this service publishes the command \
             registry; it does not execute commands",
        )))
    }

    fn execute_interactive(
        self: std::rc::Rc<Self>,
        _params: cli_service::ExecuteInteractiveParams,
        _results: cli_service::ExecuteInteractiveResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(Err(super::errors::decline(
            crate::core::ErrorCode::NotSupported,
            "notSupported: no-backend: this service publishes the command \
             registry; it does not run interactive sessions",
        )))
    }
}

/// The registry service's server, as a hook the router can route to.
pub fn server(
    state: std::sync::Arc<RegistryState>,
    schema_trees: std::sync::Arc<Mutex<Vec<(u64, &'static [u8])>>>,
) -> Box<dyn ClientHook> {
    let server = CliServer {
        state,
        schema_trees,
    };
    let client: cli_service::Client = capnp_rpc::new_client(server);
    client.into_client_hook()
}
