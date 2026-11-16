# scripts/split_cli.py - V2-A01 phase 1. Usage: python3 scripts/split_cli.py crates/cli/src [--apply]
# Mechanical mover: reads the MAP (re-derived for the current tree by scripts/split_cli_MAP.py —
# the spec-embedded MAP covered the 7,696-line tree's 85 named items; this one covers all 223
# named movable items plus the 17 impl blocks of the 19,308-line tree), moves each item VERBATIM
# to its target module file, bumps visibility to pub(crate) for non-pub fn/struct/enum items,
# prepends `//! Moved verbatim from lib.rs by scripts/split_cli.py (V2-A01).` + `use crate::*;`
# to each new module, rewrites lib.rs to drop the moved items and append `mod X; pub use X::*;`
# declarations so existing names keep resolving. `pub use` (not `pub(crate) use`) is deliberate:
# glob re-exports cap at each item's own visibility, so the items main.rs needs (`Cli`, `run`,
# `apply_startup_env`) stay pub at the crate root and private items stay crate-internal —
# per-item visibility is preserved exactly. The existing `serve` top is appended to (its
# serve/mod.rs already exists); the other nested tops get fresh mod.rs roots.
import re, sys, os, collections
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from split_items import index
NL = chr(10)
MAP = {'args': ['SecretsOp', 'Cli', 'Command', 'CacheOp', 'PerfOp', 'DocsOp', 'apply_startup_env', 'run', 'run_with_writer'], 'boot_entry': ['generate_island_boot_entry', 'jsx_import_source_for', 'inject_island_boot_script'], 'build/cmd': ['cmd_build'], 'build/secret': ['build_secret'], 'bundle_env': ['framework_prebundle_extras', 'default_ssr_externals', 'BundleEnvArgs', 'bundle_env_options'], 'commands/attest': ['cmd_attest_write', 'cmd_verify', 'IntegrityVerdict', 'integrity_verdict', 'signature_keyid', 'resolve_pubkey', 'rebuild_and_compare', 'rebuild_in', 'copy_project', 'config_fingerprint', 'git_output'], 'commands/author': ['cmd_add', 'cmd_generate', 'cmd_migrate', 'cmd_schema', 'cmd_config', 'find_config_file'], 'commands/docs': ['cmd_docs'], 'commands/fmt': ['resolve_tool', 'typecheck_backend_plan', 'run_tool', 'cmd_fmt'], 'commands/inspect': ['project_facts_from_tree', 'has_rust_fn_sources', 'capability_row_lines', 'write_routes_reasons', 'scan_routes_with_plugins', 'cmd_routes', 'cmd_analyze', 'cmd_check', 'which', 'cmd_why', 'cmd_why_route', 'find_route_entry', 'cmd_why_module', 'cmd_doctor'], 'commands/ops': ['cmd_secrets', 'cmd_cache', 'cmd_audit', 'cmd_mcp', 'cmd_test'], 'commands/perf': ['cmd_perf_bisect', 'git_tree_dirty'], 'commands/render': ['cmd_render'], 'commands/replay': ['ReplayHttpExecutor', 'ReplayReport', 'run_replay', 'cmd_replay'], 'config_load': ['load_config_or_default', 'load_config_strict', 'has_explicit_render_config', 'refine_render_modes_from_ast', 'scan_routes', 'param_name', 'pattern_of', 'walk_files', 'read_json', 'config_rules_to_router'], 'data_route': ['RouteDataArgs', 'route_data_response', 'RouteDataInnerArgs', 'route_data_response_inner'], 'dev/client_bundle': ['islands_json_source', 'canonical_route_id', 'ensure_dev_client_bundle', 'build_client_bundle', 'is_client_source', 'content_source_changed', 'consolidate_tailwind_css_and_manifest'], 'dev/cmd': ['rebuild_stale_workspace_packages', 'install_dev_log_file', 'cmd_dev', 'start_dev_server_full', 'start_dev_server'], 'dev/hmr': ['hmr_route_data_update', 'canonicalize_manifest_key', 'compute_layout_chain', 'enrich_manifest_layout_chains', 'populate_dev_component_source', 'HmrModuleGraph', 'build_hmr_graph'], 'dispatch/entry': ['SwappableRouteTree', 'DispatchOpts', 'build_dispatch', 'forward_action_set_cookies', 'build_params_json', 'DispatchArgs', 'build_dispatch_swappable'], 'emit/assets': ['copy_tree', 'extract_img_w_spec', 'extract_svg_react_spec'], 'emit/config_bundle': ['emit_config_bundle'], 'emit/edge_wasm': ['compile_edge_wasm', 'find_edge_entry', 'bundle_edge_entry'], 'emit/fonts': ['emit_fonts', 'harvest_used_chars', 'collect_html_sources'], 'emit/i18n': ['stamp_i18n_locales', 'scan_message_keys', 'find_literal_line'], 'emit/pwa': ['emit_pwa_worker', 'emit_web_manifest'], 'emit/search': ['write_content_atomic', 'emit_search_engine_index', 'refresh_search_index', 'emit_content_search_indexes'], 'emit/sitemap': ['emit_sitemap_and_robots'], 'emit/snapshot': ['persist_diagnostics', 'emit_snapshot_lint', 'emit_snapshot_blob'], 'endpoints/dispatchers': ['AssetsImageBackend', 'SvgOgBackendImpl', 'ServerFnDispatcherImpl', 'build_server_fn_dispatcher', 'build_js_server_fn_dispatcher', 'mount_native_fn_source', 'NativeFnSourceImpl', 'ActionDispatcherImpl', 'build_action_dispatcher', 'EndpointDispatcherImpl', 'cfg_socket_routes', 'build_endpoint_dispatcher', 'MiddlewareDispatcherImpl', 'build_middleware_dispatcher', 'SocketDispatcherImpl', 'build_socket_dispatcher', 'build_search_endpoint'], 'endpoints/image_og': ['build_og_endpoint', 'build_image_endpoint'], 'hooks': ['confine_plugin_path', 'plugins_allow_absolute', 'build_css_transform_hook', 'fold_plugin_map', 'compose_transform_outputs', 'build_js_plugin_hook', 'build_transform_hook', 'load_client_assets', 'collect_plugin_paths_for_hook', 'build_js_plugin_resolve_hook', 'build_js_plugin_load_hook', 'collect_plugin_options', 'build_js_plugin_render_chunk_hook', 'build_js_plugin_generate_bundle_hook', 'build_js_plugin_transform_html_hook', 'hook_result_str', 'extract_hook_map', 'hook_result_str_array', 'call_js_plugin_hook', 'fire_lifecycle_hook', 'plugin_declares_any', 'apply_config_patch', 'plugin_render_head', 'resolve_cache_handler_via_plugin', 'neutral_head_to_html', 'html_escape', 'normalize_route_meta', 'merge_neutral_head'], 'observability': ['init_tracing', 'env_filter', 'build_otel_layer'], 'prerender': ['PrerenderedShells', 'url_path_from_rel', 'boot_script_tag', 'manifest_integrity_for', 'prerender_locales', 'prefixed_prerender_path', 'PrerenderTarget', 'PrerenderTargetsArgs', 'prerender_targets', 'PrerenderArgs', 'filter_head_links', 'write_prerender', 'route_source_hash', 'hash_file_into', 'try_reuse_prerender', 'collect_route_chunks', 'push_chunk_hash', 'strip_close_and_append_settles', 'PprShellEntry', 'PprShellStore', 'ppr_tap_stream', 'ppr_filter_stream', 'find_subslice'], 'serve/session': ['parallel_load_savings_ms', 'session_ttl_secs', 'set_session_ttl', 'encode_session_cookie'], 'serve/start': ['DevFlags', 'StartedServer', 'wrap_dispatch_with_stale_guard', 'stale_503', 'start_server_inner', 'resolve_bind_host', 'start_server_inner_full', 'resolve_port', 'cmd_serve'], 'serve/stream': ['start_server', 'render_stream_to_body_stream', 'render_stream_with_recovery', 'probe_render_error_status', 'chunk_signals_error', 'str_signals_error', 'parse_form_urlencoded', 'action_from_query']}
IMPLS = [('12481-12535', 'endpoints/dispatchers'), ('12547-12559', 'endpoints/dispatchers'), ('12652-12692', 'endpoints/dispatchers'), ('12830-12850', 'endpoints/dispatchers'), ('12857-12918', 'endpoints/dispatchers'), ('12935-13004', 'endpoints/dispatchers'), ('13072-13120', 'endpoints/dispatchers'), ('13137-13161', 'endpoints/dispatchers'), ('15799-15812', 'commands/attest'), ('16757-16844', 'commands/replay'), ('18983-19054', 'prerender'), ('5533-5552', 'prerender'), ('5554-5558', 'emit/pwa'), ('562-600', 'args'), ('9228-9245', 'serve/start'), ('9311-9318', 'serve/stream'), ('9606-9622', 'dispatch/entry')]
VIS = re.compile(r'^(pub([(][a-z]+[)])?[ ]+)?(async[ ]+)?(unsafe[ ]+)?(fn|struct|enum)(?![A-Za-z0-9_])')
# Toy-crate/self-check hook: an external MAP literal file (as scripts/split_cli_MAP.py emits)
# overrides the embedded one; the default stays the embedded oxide MAP.
if os.environ.get('SPLIT_CLI_MAP'):
    _ns = {}
    exec(open(os.environ['SPLIT_CLI_MAP']).read(), _ns)
    MAP, IMPLS = _ns['MAP'], _ns['IMPLS']
PREAMBLE = ('//! Moved verbatim from lib.rs by scripts/split_cli.py (V2-A01).' + NL + NL
            + 'use crate::*;' + NL + NL)

def main():
    src_dir = sys.argv[1]
    apply = '--apply' in sys.argv
    path = os.path.join(src_dir, 'lib.rs')
    text = open(path).read()
    lines = text.split(NL)
    items = index(text)
    by_name = collections.defaultdict(list)
    for s, e, k, nm in items:
        if k in ('fn', 'struct', 'enum') and nm:
            by_name[nm].append((s, e))
    want = [n for ns in MAP.values() for n in ns]
    missing = [n for n in want if n not in by_name]
    dup = [n for n in want if len(by_name.get(n, [])) > 1]
    unmapped = [nm for nm in by_name if nm not in set(want)]
    print('missing:', missing, 'duplicates:', dup, 'unmapped:', unmapped)
    assert not missing and not dup and not unmapped, 'MAP drifted from the tree: fix the names'
    moved_lines = sum(by_name[n][0][1] - by_name[n][0][0] + 1 for n in want)
    impl_spans = {}
    for span, mod in IMPLS:
        s, e = (int(x) for x in span.split('-'))
        impl_spans[(s, e)] = mod
    indexed_impls = {(s, e): nm for s, e, k, nm in items if k == 'impl'}
    bad = [sp for sp in impl_spans if sp not in indexed_impls]
    assert not bad, 'impl spans drifted from the tree: re-run scripts/split_cli_MAP.py'
    print('items to move:', len(want), 'named +', len(impl_spans), 'impl blocks; lines:',
          moved_lines + sum(e - s + 1 for s, e in impl_spans), 'of', len(lines))
    if not apply:
        return
    drop = set()
    for n in want:
        s, e = by_name[n][0]
        drop.update(range(s, e + 1))
    for (s, e) in impl_spans:
        drop.update(range(s, e + 1))
    chunks = collections.defaultdict(list)
    has_pub = collections.defaultdict(bool)
    for mod, names in MAP.items():
        for n in names:
            s, e = by_name[n][0]
            body = lines[s:e + 1]
            vis = 'priv'
            for i, l in enumerate(body):
                if VIS.match(l):
                    if re.match(r'^pub (async |unsafe |fn|struct|enum)', l):
                        vis = 'pub'
                    elif not l.startswith('pub'):
                        body[i] = 'pub(crate) ' + l
                        vis = 'priv'
                    else:
                        vis = 'restricted'
                    break
            if vis == 'pub':
                has_pub[mod] = True
            chunks[mod].append((s, NL.join(body)))
    for (s, e), mod in impl_spans.items():
        chunks[mod].append((s, NL.join(lines[s:e + 1])))
    for mod, cs in chunks.items():
        cs.sort(key=lambda t: t[0])
        fp = os.path.join(src_dir, mod + '.rs')
        os.makedirs(os.path.dirname(fp), exist_ok=True)
        open(fp, 'w').write(PREAMBLE + (NL + NL).join(t for _, t in cs) + NL)
    keep = [l for i, l in enumerate(lines) if i not in drop]
    tops = sorted({m.split('/')[0] for m in MAP})
    decl = ['// V2-A01: module tree; moved items are re-exported so existing names keep resolving.']

    def rexport(vis):
        # `pub use` re-exports every item at its own visibility (pub items stay pub for
        # main.rs; pub(crate) items stay crate-internal). A `pub use` glob whose module
        # holds nothing pub enough warns `unused_imports` under -D warnings, so modules
        # with no originally-pub item get the `pub(crate) use` spelling instead.
        return 'pub' if vis else 'pub(crate)'

    for t in tops:
        kids = sorted({m.split('/')[1] for m in MAP if m.startswith(t + '/')})
        if t == 'serve':
            # serve/mod.rs already exists (brownout/generations/live/streams); append the new kids.
            smp = os.path.join(src_dir, 'serve', 'mod.rs')
            cur = open(smp).read()
            add = NL.join('mod %s;%s%s use %s::*;' % (k, NL, rexport(has_pub[t + '/' + k]), k) for k in kids)
            open(smp, 'w').write(cur.rstrip(NL) + NL + NL + add + NL)
            decl += ['pub use serve::*;']
        elif kids:
            decl += ['mod %s;' % t, rexport(any(has_pub[t + '/' + k] for k in kids)) + ' use %s::*;' % t]
            open(os.path.join(src_dir, t, 'mod.rs'), 'w').write('//! V2-A01 module root.' + NL + NL
                                                                + NL.join('mod %s;%s%s use %s::*;' % (k, NL, rexport(has_pub[t + '/' + k]), k) for k in kids) + NL)
        else:
            decl += ['mod %s;' % t, rexport(has_pub[t]) + ' use %s::*;' % t]
    open(path, 'w').write(NL.join(keep).rstrip(NL) + NL + NL + NL.join(decl) + NL)
    print('applied; now run: cargo fmt -p oxide-fw-cli && cargo check -p oxide-fw-cli')

if __name__ == '__main__':
    main()
