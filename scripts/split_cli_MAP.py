# scripts/split_cli_MAP.py - V2-A01 phase 1 MAP re-derivation for the current tree.
# The spec's embedded MAP targeted the 7,696-line tree (85 named items). Today's lib.rs is
# 19,308 lines with 285 top-level items, so the MAP is re-derived: the spec MAP verbatim
# (two stale names dropped, recorded in the PR body), extended with explicit rule buckets
# for every new family, and an anchor-nearest fallback for whatever remains. Impl blocks
# are span-keyed (they have no stable name) and go next to their sibling build_*/struct.
# Usage: python3 scripts/split_cli_MAP.py  -> prints the MAP/IMPLS literals + a validation
# summary. The literals are embedded in scripts/split_cli.py (the spec-embedded mover).
import sys, os, collections
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from split_items import index

LIB = os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(__file__))), 'crates/cli/src/lib.rs')

# The spec-embedded MAP (docs/spec-v0.2-draft.md, WP-A01 step 1) verbatim. Two names the
# F-passes retired from the tree (`is_secret_key`, `log_request`) are dropped — the
# re-derivation records that delta; every other entry keeps its spec target.
SPEC_MAP = {
 'args': ['SecretsOp', 'Command', 'CacheOp'],
 'observability': ['init_tracing', 'env_filter', 'build_otel_layer'],
 'config_load': ['load_config_or_default', 'load_config_strict', 'has_explicit_render_config',
                 'refine_render_modes_from_ast', 'scan_routes', 'param_name', 'pattern_of',
                 'walk_files', 'read_json', 'config_rules_to_router'],
 'boot_entry': ['generate_island_boot_entry', 'jsx_import_source_for', 'inject_island_boot_script'],
 'bundle_env': ['bundle_env_options'],
 'dev/hmr': ['hmr_route_data_update'],
 'dev/client_bundle': ['ensure_dev_client_bundle'],
 'dev/cmd': ['cmd_dev', 'start_dev_server_full', 'start_dev_server'],
 'emit/edge_wasm': ['compile_edge_wasm'],
 'emit/search': ['emit_content_search_indexes'],
 'emit/sitemap': ['emit_sitemap_and_robots'],
 'emit/fonts': ['emit_fonts'],
 'emit/snapshot': ['persist_diagnostics', 'emit_snapshot_lint', 'emit_snapshot_blob'],
 'emit/config_bundle': ['emit_config_bundle'],
 'emit/pwa': ['emit_pwa_worker', 'emit_web_manifest'],
 'prerender': ['url_path_from_rel', 'write_prerender'],
 'data_route': ['route_data_response'],
 'build/secret': ['build_secret'],
 'build/cmd': ['cmd_build'],
 'serve/stream': ['start_server', 'render_stream_to_body_stream', 'render_stream_with_recovery',
                  'probe_render_error_status', 'chunk_signals_error', 'parse_form_urlencoded', 'action_from_query'],
 'dispatch/entry': ['build_dispatch'],
 'endpoints/image_og': ['build_og_endpoint', 'build_image_endpoint'],
 'endpoints/dispatchers': ['build_server_fn_dispatcher', 'build_action_dispatcher', 'cfg_socket_routes',
                           'build_endpoint_dispatcher', 'build_middleware_dispatcher', 'build_socket_dispatcher'],
 'hooks': ['build_css_transform_hook', 'build_js_plugin_hook', 'build_transform_hook', 'load_client_assets'],
 'serve/start': ['start_server_inner', 'start_server_inner_full', 'resolve_bind_host', 'resolve_port', 'cmd_serve'],
 'commands/inspect': ['cmd_routes', 'cmd_analyze', 'cmd_check', 'which', 'cmd_why', 'cmd_doctor'],
 'commands/ops': ['cmd_secrets', 'cmd_cache', 'cmd_audit', 'cmd_test', 'cmd_mcp'],
 'commands/author': ['cmd_add', 'cmd_generate', 'cmd_migrate', 'cmd_schema', 'cmd_config', 'find_config_file'],
 'commands/render': ['cmd_render'],
}

# Extension rules: every new family in the current tree, named the same way the embedded
# MAP names modules (<family>/<concern> or a flat concern). Kept name-keyed so the mover
# stays mechanical.
RULES = {
 'args': ['Cli', 'PerfOp', 'DocsOp', 'run', 'run_with_writer', 'apply_startup_env'],
 'dev/hmr': ['populate_dev_component_source', 'HmrModuleGraph', 'build_hmr_graph'],
 'dev/client_bundle': ['build_client_bundle', 'is_client_source', 'content_source_changed',
                       'consolidate_tailwind_css_and_manifest', 'islands_json_source'],
 'dev/cmd': ['rebuild_stale_workspace_packages', 'install_dev_log_file'],
 'bundle_env': ['framework_prebundle_extras', 'default_ssr_externals', 'BundleEnvArgs'],
 'emit/search': ['write_content_atomic', 'emit_search_engine_index', 'refresh_search_index'],
 'emit/fonts': ['harvest_used_chars', 'collect_html_sources'],
 'emit/i18n': ['stamp_i18n_locales', 'scan_message_keys', 'find_literal_line'],
 'emit/assets': ['extract_img_w_spec', 'extract_svg_react_spec', 'copy_tree'],
 'prerender': ['PrerenderedShells', 'boot_script_tag', 'manifest_integrity_for', 'prerender_locales',
               'prefixed_prerender_path', 'PrerenderTarget', 'PrerenderTargetsArgs', 'prerender_targets',
               'PrerenderArgs', 'filter_head_links', 'route_source_hash', 'hash_file_into',
               'try_reuse_prerender', 'collect_route_chunks', 'push_chunk_hash',
               'strip_close_and_append_settles', 'PprShellEntry', 'PprShellStore',
               'ppr_tap_stream', 'ppr_filter_stream', 'find_subslice'],
 'data_route': ['RouteDataArgs', 'RouteDataInnerArgs', 'route_data_response_inner'],
 'emit/edge_wasm': ['find_edge_entry', 'bundle_edge_entry'],
 'serve/session': ['session_ttl_secs', 'set_session_ttl', 'encode_session_cookie'],
 'serve/start': ['DevFlags', 'StartedServer', 'wrap_dispatch_with_stale_guard', 'stale_503'],
 'serve/stream': ['str_signals_error'],
 'dispatch/entry': ['SwappableRouteTree', 'DispatchOpts', 'DispatchArgs', 'build_dispatch_swappable',
                    'forward_action_set_cookies', 'build_params_json'],
 'endpoints/dispatchers': ['AssetsImageBackend', 'SvgOgBackendImpl', 'ServerFnDispatcherImpl',
                           'build_js_server_fn_dispatcher', 'mount_native_fn_source', 'NativeFnSourceImpl',
                           'ActionDispatcherImpl', 'EndpointDispatcherImpl', 'MiddlewareDispatcherImpl',
                           'SocketDispatcherImpl', 'build_search_endpoint'],
 'hooks': ['confine_plugin_path', 'plugins_allow_absolute', 'fold_plugin_map', 'compose_transform_outputs',
           'collect_plugin_paths_for_hook', 'build_js_plugin_resolve_hook', 'build_js_plugin_load_hook',
           'collect_plugin_options', 'build_js_plugin_render_chunk_hook',
           'build_js_plugin_generate_bundle_hook', 'build_js_plugin_transform_html_hook',
           'hook_result_str', 'extract_hook_map', 'hook_result_str_array', 'call_js_plugin_hook',
           'fire_lifecycle_hook', 'plugin_declares_any', 'apply_config_patch', 'plugin_render_head',
           'resolve_cache_handler_via_plugin', 'neutral_head_to_html', 'html_escape',
           'normalize_route_meta', 'merge_neutral_head'],
 'commands/inspect': ['project_facts_from_tree', 'has_rust_fn_sources', 'capability_row_lines',
                      'write_routes_reasons', 'scan_routes_with_plugins', 'cmd_why_route',
                      'find_route_entry', 'cmd_why_module'],
 'commands/attest': ['cmd_attest_write', 'IntegrityVerdict', 'integrity_verdict', 'signature_keyid',
                     'resolve_pubkey', 'rebuild_and_compare', 'rebuild_in', 'copy_project',
                     'config_fingerprint', 'git_output', 'cmd_verify'],
 'commands/replay': ['ReplayHttpExecutor', 'ReplayReport', 'run_replay', 'cmd_replay'],
 'commands/fmt': ['resolve_tool', 'typecheck_backend_plan', 'run_tool', 'cmd_fmt'],
 'commands/perf': ['cmd_perf_bisect', 'git_tree_dirty'],
 'commands/docs': ['cmd_docs'],
}

def main():
    text = open(LIB).read()
    its = index(text)
    named = [(s, e, k, nm) for s, e, k, nm in its if k in ('fn', 'struct', 'enum') and nm]
    impls = [(s, e, k, nm) for s, e, k, nm in its if k == 'impl']
    by_name = {nm: (s, e) for s, e, k, nm in named}
    dup = [nm for nm, c in collections.Counter(nm for _, _, _, nm in named).items() if c > 1]

    spec_names = [n for ns in SPEC_MAP.values() for n in ns]
    missing = [n for n in spec_names if n not in by_name]
    spec_live = {m: [n for n in ns if n in by_name] for m, ns in SPEC_MAP.items()}

    covered = {}
    for m, ns in spec_live.items():
        for n in ns: covered[n] = m
    for m, ns in RULES.items():
        for n in ns:
            if n not in by_name:
                print('RULE names absent from tree:', m, n); sys.exit(1)
            if n in covered:
                print('RULE name double-assigned:', n, covered[n], m); sys.exit(1)
            covered[n] = m

    # anchor-nearest fallback: unassigned named items take the module of the nearest
    # assigned item (by start-line distance, either direction).
    anchors = sorted((by_name[n][0], m) for n, m in covered.items())
    def nearest(s):
        best, bd = None, None
        for a, m in anchors:
            d = abs(a - s)
            if bd is None or d < bd: best, bd = m, d
        return best
    leftover = [nm for s, e, k, nm in named if nm not in covered]
    for nm in sorted(leftover, key=lambda n: by_name[n][0]):
        m = nearest(by_name[nm][0])
        covered[nm] = m
        anchors.append((by_name[nm][0], m)); anchors.sort()

    # impl blocks: span-keyed. The dispatch impls go to endpoints/dispatchers (spec note (c));
    # the rest join the nearest named item's module (their struct/build_* sibling).
    impl_moves = []
    for s, e, k, nm in impls:
        if 'EndpointBackend' in nm or 'Dispatcher' in nm or 'NativeFnSource' in nm:
            impl_moves.append((f'{s}-{e}', 'endpoints/dispatchers'))
            continue
        if nm == 'impl Command {':
            impl_moves.append((f'{s}-{e}', 'args'))
            continue
        impl_moves.append((f'{s}-{e}', nearest(s)))

    # validation: every named movable item covered exactly once; no unmapped left.
    unmapped = [nm for s, e, k, nm in named if nm not in covered]
    print('missing:', sorted(missing))
    print('duplicates:', dup)
    print('unmapped:', unmapped)
    assert not unmapped and not dup, 'MAP incomplete'
    MAP = collections.defaultdict(list)
    for n, m in covered.items(): MAP[m].append(n)
    MAP = {m: sorted(ns, key=lambda n: by_name[n][0]) for m, ns in MAP.items()}
    print('# named movable items:', len(named), ' impl blocks:', len(impls))
    print('# modules:', len(MAP), ' stays in lib.rs: use/type/const/static/mod items')
    for m in sorted(MAP):
        lines = sum(by_name[n][1] - by_name[n][0] + 1 for n in MAP[m])
        print('#   %-26s %3d items %6d lines' % (m, len(MAP[m]), lines))
    impl_lines = collections.Counter(m for _, m in impl_moves)
    for m, c in impl_lines.items(): print('#   impl lines ->', m, c, 'blocks')
    print('# ---- MAP literal ----')
    print('MAP = ' + repr(dict(sorted(MAP.items()))))
    print('IMPLS = ' + repr(sorted(impl_moves)))

if __name__ == '__main__':
    main()
