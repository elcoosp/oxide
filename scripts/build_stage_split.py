# scripts/build_stage_split.py - V2-A01 phase 3 (spec "Step 3. Phase 3, split
# build_dispatch"): replaces the 18-parameter build_dispatch's monolithic request
# closure with DispatchDeps + RequestCtx + ten stage functions in dispatch/, the
# spec's stage protocol (Flow comes from oxide_fw_dispatch::flow, landed with A02).
# Behavior-preserving by construction:
#   - every body line of the original closure (HEAD lines 314-2770) moves verbatim
#     into exactly one stage function (the async-pipeline spans dedent one level);
#   - per-request locals shared across stages become RequestCtx fields; a stage
#     that creates a value keeps the original `let` and hands it to the context
#     with one assignment; a later stage re-binds it (ref / ref mut / clone /
#     take) so the body text is untouched;
#   - boot handles become DispatchDeps; the closure captures one Arc<DispatchDeps>
#     and runs the stage table (run_stages) instead of holding ~30 captures;
#   - inside the nine Flow stages, `return <resp>;` rewrites to
#     `return Flow::Respond(<resp>);` (plain returns by rule; the Box::pin
#     no-await wrappers unwrap; the two await-carrying wrappers hoist via the
#     recorded substitutions). The last stage (stage_render, the sketch's
#     stage_render + stage_finish pair) returns OxideResponse directly, so its
#     returns stay verbatim.
# Wrapper lines are fenced with `// >>> stage-wrapper` / `// <<< stage-wrapper`
# markers for scripts/split_selfcheck.py --stages.
# Usage: python3 scripts/build_stage_split.py
import sys, os, re

NL = chr(10)
SRC = os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(__file__))),
                   'crates', 'cli', 'src', 'dispatch')

# HEAD-line spans of the ten stage bodies inside the request closure (1-based,
# inclusive). S1-S3 are the sync prelude (indent 8); S4-S10 are the async block
# (indent 12 -> dedent 4 into the stage fns).
SPANS = {
    'prelude': (314, 553),
    'match': (554, 631),
    'route': (633, 999),
    'middleware': (1002, 1231),
    'data_route': (1233, 1258),
    'action': (1260, 1390),
    'l0_lookup': (1392, 1581),
    'loaders': (1583, 2075),
    'endpoint': (2077, 2191),
    'render': (2193, 2770),
}
ASYNC_SPANS = {'middleware', 'data_route', 'action', 'l0_lookup', 'loaders',
               'endpoint', 'render'}
# stages that short-circuit with Flow (all but the last)
FLOW_STAGES = ['prelude', 'match', 'route', 'middleware', 'data_route', 'action',
               'l0_lookup', 'loaders', 'endpoint']

# per-stage re-binds from RequestCtx: name -> kind.
#   ref        let name = &cx.name;
#   refmut     let name = &mut cx.name;
#   copy       let name = cx.name;                      (Copy field)
#   clone      let name = cx.name.clone();              (owned copy)
#   take       let name = cx.name.take().expect(...);   (owned, Option field)
#   takemut    let mut name = cx.name.take().expect(...);
#   replace    let mut name = std::mem::replace(&mut cx.name, Default::default());
REBINDS = {
    'prelude': {
        'req': 'ref', 'swappable': 'depsref', 'generations_for_render': 'depsref',
        'esm_state': 'depsref', 'pool': 'depsclone', 'metrics': 'depsarc',
        'proxy_rules': 'depsref', 'live': 'depsref', 'archived': 'depsref',
        'streams_for_dispatch': 'depsref',
    },
    'match': {
        'req': 'ref', 'i18n_cfg': 'depsref', 'archived': 'depsref', 'tree': 'ref',
        'raw': 'clone',
    },
    'route': {
        'req': 'ref', 'tree': 'ref', 'matched': 'ref',
        'serving_pools': 'depsref', 'loader_runner': 'depsclone',
        'page_renderer': 'depsclone', 'pool': 'depsclone', 'routes_root': 'depsclone',
        'boot_script_url_for_render': 'depsclone',
        'boot_script_integrity_for_render': 'depsclone',
        'endpoint_for_dispatch': 'depsclone', 'waterfall': 'depsclone',
        'record': 'depsref', 'recordings': 'depsref',
        'brownout_for_dispatch': 'depsref', 'l0': 'depsclone',
        'actions_for_dispatch': 'depsclone',
        'vary_headers': 'depsref', 'is_data_request': 'copy',
        'normalized': 'ref', 'req_path': 'clone',
    },
    'middleware': {
        'req_owned': 'ctxreq', 'metrics': 'depsarc', 'label': 'ref',
        'params': 'optref', 'middleware_module_paths': 'clone',
        'endpoint_params_json': 'refmut', 'endpoint_body': 'ref',
        'self_dispatch_for_closure': 'depsarc',
        'session_codec': 'depsclone',
        'middleware_for_dispatch': 'depsclone', 'rewrite_hop': 'copy',
        'started': 'copy', 'req_recording': 'ref',
    },
    'data_route': {
        'req_owned': 'ctxreq', 'loader_runner': 'ref', 'module_path': 'ref',
        'route_id': 'ref', 'params': 'optref', 'metrics': 'depsarc',
        'middleware_locals': 'ref', 'middleware_headers': 'ref',
        'layout_module_paths': 'ref', 'resolved_locale': 'ref',
        'session_codec': 'depsclone', 'waterfall': 'clone', 'is_live': 'copy',
        'started': 'copy', 'middleware_ms': 'copy', 'is_data_request': 'copy',
    },
    'action': {
        'req_owned': 'ctxreq', 'action_name': 'ref', 'module_path': 'ref',
        'action_dispatcher': 'ref', 'action_params': 'ref', 'action_route': 'ref',
        'middleware_locals': 'ref', 'middleware_headers': 'refmut',
        'waterfall': 'clone', 'l0': 'depsclone', 'ppr_for_request': 'depsclone',
        'live_for_request': 'depsclone', 'metrics': 'depsarc', 'label': 'ref',
        'started': 'copy', 'middleware_ms': 'copy',
    },
    'l0_lookup': {
        'req_owned': 'ctxreq', 'l0_eligible': 'copy', 'l0': 'depsclone',
        'brownout_ceiling': 'copy', 'l0_route': 'ref', 'l0_params': 'ref',
        'l0_vary': 'ref', 'l0_path': 'clone', 'flight_key': 'ref',
        'flight_map': 'depsclone', 'ppr_for_request': 'depsclone',
        'middleware_headers': 'clone',
        'metrics': 'depsarc', 'label': 'ref', 'started': 'copy',
        'request_nonce': 'ref', 'waterfall': 'clone',
    },
    'loaders': {
        'req_owned': 'ctxreq', 'loader_runner': 'ref', 'module_path': 'ref',
        'route_id': 'ref', 'params': 'optref', 'has_server_module': 'copy',
        'needs_parent': 'copy', 'label': 'ref', 'metrics': 'depsarc',
        'middleware_locals': 'ref', 'session_codec': 'depsclone',
        'resolved_locale': 'ref', 'layout_module_paths': 'ref',
        'waterfall': 'clone', 'started': 'copy', 'session_before': 'ref',
    },
    'endpoint': {
        'req_owned': 'ctxreq', 'endpoint_for_dispatch': 'clone',
        'endpoint_module_path': 'ref', 'endpoint_params_json': 'refmut',
        'endpoint_body': 'ref', 'params': 'optref', 'middleware_locals': 'ref',
        'middleware_headers': 'clone', 'streams_for_dispatch': 'depsref',
        'stream_counter': 'depsarc', 'req_recording': 'ref', 'metrics': 'depsarc',
        'label': 'ref', 'started': 'copy',
    },
    'render': {
        'req_owned': 'ctxreq', 'page_renderer': 'ref', 'route_id_for_render': 'ref',
        'layout_ids': 'ref', 'unmatched_route_id': 'ref',
        'unmatched_layout_ids': 'ref', 'params': 'take', 'data': 'takemut',
        'action_form': 'takeoptmut', 'page_settles': 'takeoptmut', 'route_meta': 'ref',
        'loader_set_cookies': 'take', 'loader_headers': 'take',
        'deferred_values': 'ref', 'error_route_id': 'ref', 'resolved_locale': 'ref',
        'request_nonce': 'ref', 'middleware_headers': 'clone', 'waterfall': 'clone',
        'boot_script_url_for_render': 'ref', 'boot_script_integrity_for_render': 'ref',
        'ppr_for_request': 'depsclone', 'l0': 'depsclone', 'l0_eligible': 'copy',
        'l0_route': 'ref', 'l0_params': 'ref', 'l0_vary': 'ref', 'l0_path': 'ref',
        'flight_key': 'ref', 'flight_owner': 'replace', 'metrics': 'depsarc',
        'label': 'ref', 'status': 'copy', 'stage_load_start': 'copy',
        'boot_build_id': 'clone', 'started': 'copy', 'middleware_ms': 'copy',
        'req_recording': 'ref',
    },
}

# stages that CREATE each ctx field -> the hand-off assignment (Some-wrap for
# Option fields, bare for plain fields).
HANDOFFS = {
    'prelude': [('tree', 'bare'), ('boot_build_id', 'bare'), ('rewrite_hop', 'bare'),
                ('raw', 'bare')],
    'match': [
        ('normalized', 'bare'), ('req_path', 'bare'), ('resolved_locale', 'bare'),
        ('is_data_request', 'bare'), ('matched', 'bare'), ('status', 'bare'),
        ('label', 'bare'),
    ],
    'route': [
        ('loader_runner', 'bare'), ('page_renderer', 'bare'),
        ('module_path', 'bare'), ('route_id', 'bare'), ('params', 'Some'),
        ('has_server_module', 'bare'), ('render_mode', 'bare'),
        ('needs_parent', 'bare'), ('is_live', 'bare'),
        ('endpoint_module_path', 'bare'), ('middleware_module_paths', 'bare'),
        ('endpoint_params_json', 'bare'), ('endpoint_body', 'bare'),
        ('request_nonce', 'bare'), ('unmatched_route_id', 'bare'),
        ('unmatched_layout_ids', 'bare'), ('error_route_id', 'bare'),
        ('route_id_for_render', 'bare'), ('layout_ids', 'bare'),
        ('layout_module_paths', 'bare'), ('action_name', 'bare'),
        ('action_dispatcher', 'bare'), ('action_params', 'bare'),
        ('action_route', 'bare'), ('req_recording', 'bare'),
        ('endpoint_for_dispatch', 'bare'),
        ('boot_script_url_for_render', 'bare'),
        ('boot_script_integrity_for_render', 'bare'), ('waterfall', 'bare'),
        ('brownout_ceiling', 'bare'), ('l0_eligible', 'bare'), ('l0_route', 'bare'),
        ('l0_params', 'bare'), ('l0_vary', 'bare'), ('l0_path', 'bare'),
        ('flight_key', 'bare'),
    ],
    'middleware': [('middleware_locals', 'bare'), ('middleware_headers', 'bare'),
                   ('middleware_ms', 'bare'), ('session_before', 'bare')],
    'action': [('action_form', 'bare')],
    'l0_lookup': [('flight_owner', 'bare')],
    'loaders': [
        ('data', 'Some'), ('page_settles', 'bare'), ('route_meta', 'bare'),
        ('loader_set_cookies', 'Some'), ('loader_headers', 'Some'),
        ('deferred_values', 'bare'), ('stage_load_start', 'bare'),
    ],
    'data_route': [], 'endpoint': [], 'render': [],
}

# ctx fields whose hand-off wraps in Some (the field is Option<T> because a
# later stage moves the value out) — everything else hands off bare.
SOME_HANDOFF = {'params', 'data', 'page_settles',
                'loader_set_cookies', 'loader_headers'}

# The two await-carrying Box::pin short-circuits (the generic return-wrap rule
# cannot lift an async block that awaits): recorded hoists. Each entry: the
# HEAD line range it replaces and the replacement text (shared verbatim by the
# generator and split_selfcheck --stages, so the proof cannot drift).
HOIST_TEXT = {
    355: [
        '                Some(crate::esm_dev::EsmAction::PoolModule(rel)) => {',
        '                    let esm = std::sync::Arc::clone(esm);',
        '                    let pool = pool.clone();',
        '                    let resp = crate::esm_dev::serve_pool_compiled_module(&esm, &rel,',
        '                        pool.as_ref())',
        '                        .await;',
        '                    return Flow::Respond(resp);',
        '                }',
    ],
    405: [
        '                let resp = match oxide_fw_server::forward_proxy(&url, req_owned).await {',
        '                    Ok(r) => {',
        '                        metrics_proxy.observe("_proxy", started.elapsed(), r.status);',
        '                        r',
        '                    }',
        '                    Err(e) => {',
        '                        tracing::warn!(rule = %m_rule, error = %e, "proxy forward failed");',
        '                        let r = oxide_fw_server::OxideResponse::new(',
        '                            502,',
        '                            format!("proxy upstream error: {}", e.0.message),',
        '                        );',
        '                        metrics_proxy.observe("_proxy", started.elapsed(), 502);',
        '                        r',
        '                    }',
        '                };',
        '                return Flow::Respond(resp);',
    ],
}

# single-line / small recorded substitutions (HEAD line -> replacement text or
# None to drop).
SUBS = {
    # the live arm's local helper: Flow stages return OxideResponse, not
    # BoxedResponse, so the helper's signature loses the pin wrapper.
    435: [
        '            let not_accepted = |status: u16, msg: &\'static str| ->',
        '                oxide_fw_server::OxideResponse {',
        '                oxide_fw_server::OxideResponse::html(status, msg)',
        '            };',
    ],
    436: None,
    437: None,
    # stage_action mutates middleware_headers through the re-bind, so the
    # for-loop borrows via .iter() (a `&(&mut Vec)` never iterates).
    1371: [
        '                for (k, v) in middleware_headers.iter() {',
    ],
    873: [
        '            .and_then(|c| crate::record_wire::RequestRecording::start(c, recordings, req_owned));',
    ],
    1384: [
        '                    forward_action_set_cookies(&resp.headers, middleware_headers);',
    ],
    # route: the render-mode mutation lives in the later stages through the
    # context re-binds, so the creating stage only initializes.
    762: [
        '        let endpoint_params_json: Option<serde_json::Map<String, serde_json::Value>> =',
        '            None;',
    ],
    # the old closure's setup shadow-clones are dead in the staged shape —
    # the later stages re-bind from DispatchDeps / RequestCtx directly.
    886: None, 887: None, 890: None, 892: None, 895: None, 898: None,
    863: None, 900: None, 906: None, 907: None, 994: None, 995: None, 999: None,
}

# capture-name -> DispatchDeps field name (the phase-2 setup kept underscored
# aliases; the deps struct carries the DispatchArgs field names).
DEPS_FIELD = {
    'streams_for_dispatch': 'streams',
    'brownout_for_dispatch': 'brownout',
    'actions_for_dispatch': 'actions',
    'endpoint_for_dispatch': 'endpoint_dispatcher',
    'middleware_for_dispatch': 'middleware_dispatcher',
    'generations_for_render': 'generations',
    'ppr_for_render': 'ppr_shells',
    'self_dispatch_for_closure': 'self_dispatch',
    'ppr_for_request': 'ppr_shells',
    'live_for_request': 'live',
    'boot_script_url_for_render': 'boot_script_url',
    'boot_script_integrity_for_render': 'boot_script_integrity',
}

STAGE_ORDER = ['prelude', 'match', 'route', 'middleware', 'data_route', 'action',
               'l0_lookup', 'loaders', 'endpoint', 'render']

STAGE_DOCS = {
    'prelude': ('stage_prelude — the sync request prelude (spec WP-A01 phase 3,\n'
                "/// the sketch's `stage_prelude` + its dev intercepts): per-request tree\n"
                '/// load, boot-id read, rewrite-hop guard, the ESM dev intercept, the pool\n'
                '/// gauge refresh, the §13.5 proxy arm, the F05 live endpoint, the F07\n'
                '/// stream-reattach route. Every arm short-circuits through `Flow`.'),
    'match': ('stage_match — path normalization + route match (the sketch\'s\n'
              '/// `stage_match`): trailing-slash fold, BUG-01 path tag, §16.4 locale\n'
              '/// resolution, the PERF-01 `/__data` target detection, candidate match\n'
              '/// over the archived or live tree, status/label.'),
    'route': ('stage_route — per-route input precompute + the F10 generation\n'
              "/// routing + the F12 brownout decision + the §11 L0 keying (the hot-path\n"
              '/// hoists B03 targets): loader inputs, endpoint module path, middleware\n'
              '/// chain paths, request nonce, not-found/error boundaries, layout chains,\n'
              '/// action eligibility, the recording capture and the single-flight key.'),
    'middleware': ('stage_middleware — the §8.5 ancestor middleware chain (the\n'
                   "/// sketch's `stage_middleware`): session decode, B-22 resume, the\n"
                   '/// dispatcher walk with locals/headers merge, rewrite re-dispatch,\n'
                   '/// B-05 session write-back, PERF-06 timing.'),
    'data_route': ('stage_data_route — the §8.4 `/__data` endpoint (the sketch\'s\n'
                   '/// `stage_data_route`): the loader-only route-data response with the\n'
                   '/// same middleware locals/headers the SSR render saw.'),
    'action': ('stage_action — the §9.3 form action arm (the sketch\'s\n'
               '/// `stage_action`): run_action, tag/path invalidations, the PPR/L0\n'
               '/// invalidation fan-out, B6 cookie forwarding, the 200 fall-through.'),
    'l0_lookup': ('stage_l0_lookup — the §11.1 L0 cache-hit arm + PERF-04 single\n'
                  '/// flight (the sketch\'s `stage_l0_lookup`): the F12 stale-ceiling\n'
                  '/// lookup, 304 handling, P-05 stale-while-revalidate, the compressed\n'
                  '/// materialization and the follower coalescing.'),
    'loaders': ('stage_loaders — the §9.1/§9.2 loader phase (the sketch\'s\n'
                '/// `stage_loaders`): the sequential layout chain, the parallel\n'
                '/// last-layout+page join, streaming settles, session persistence and\n'
                '/// the data/deferred/meta threading into the render phase.'),
    'endpoint': ('stage_endpoint — the §8.1 route.ts endpoint arm (the sketch\'s\n'
                 '/// `stage_endpoint`): the dispatcher call, the F07 resumable tee and\n'
                 '/// the F15 recording tee.'),
    'render': ('stage_render — the §7 render + finish pipeline (the sketch\'s\n'
               '/// `stage_render` + `stage_finish` pair, merged because the finish work\n'
               '/// (Server-Timing, header application, the F15 tee) is interleaved with\n'
               '/// the response construction): the streamed render with the error-status\n'
               '/// probe, ODF hydration payload, PPR shells, the L0 tee-and-store and\n'
               '/// the placeholder arm. The LAST stage: returns `OxideResponse` directly\n'
               '/// instead of `Flow` (there is nothing left to continue to).'),
}

# ---------------------------------------------------------------- machinery
def rebind_line(name, how):
    if how == 'skip':
        return None
    if how == 'ref':
        return '    let %s = &cx.%s;' % (name, name)
    if how == 'refmut':
        return '    let %s = &mut cx.%s;' % (name, name)
    if how == 'copy':
        return '    let %s = cx.%s;' % (name, name)
    if how == 'clone':
        return '    let %s = cx.%s.clone();' % (name, name)
    if how == 'optref':
        return '    let %s = cx.%s.as_ref().expect("the stage order sets %s");' % (name, name, name)
    if how == 'take':
        return '    let %s = cx.%s.take().expect("the stage order sets %s");' % (name, name, name)
    if how == 'takemut':
        return '    let mut %s = cx.%s.take().expect("the stage order sets %s");' % (name, name, name)
    if how == 'takeoptmut':
        # the field's value is itself an Option (the creating stage handed off
        # a `Some(x)`-shaped local bare) — take() without expect: None is a
        # legitimate state ("the arm never ran").
        return '    let mut %s = cx.%s.take();' % (name, name)
    if how == 'replace':
        # the field's value is itself an Option (an inner None is legitimate:
        # "no single-flight owner") — mem::take never panics.
        return '    let mut %s = std::mem::take(&mut cx.%s);' % (name, name)
    if how == 'ctxreq':
        return '    let req_owned = &cx.req;'
    field = DEPS_FIELD.get(name, name)
    if how == 'depsref':
        return '    let %s = &d.%s;' % (name, field)
    if how == 'depsclone':
        return '    let %s = d.%s.clone();' % (name, field)
    if how == 'depsarc':
        return '    let %s = std::sync::Arc::clone(&d.%s);' % (name, field)
    raise ValueError(how)


def transform_span(lines, lo, hi, flow_only=True):
    """Apply the recorded hoists/subs to a HEAD span slice, then wrap the
    plain returns into Flow::Respond (the identical transform runs inside
    split_selfcheck --stages, so generator and proof cannot drift)."""
    out = []
    i = lo
    while i <= hi:
        if i in HOIST_TEXT:
            out.extend(HOIST_TEXT[i])
            i += {355: 7, 405: 17}[i]
            continue
        if i in SUBS:
            rep = SUBS[i]
            if rep is None:
                i += 1
            else:
                out.extend(rep)
                i += 1
            continue
        out.append(lines[i - 1])
        i += 1
    return wrap_returns(out, flow_only)


def wrap_returns(lines, flow_only=True):
    res = []
    i, n = 0, len(lines)
    while i < n:
        l = lines[i]
        s = l.strip()
        if (flow_only and s.startswith('return ')
                and 'Flow::Respond(' not in s and s != 'return;'):
            indent = l[:len(l) - len(l.lstrip())]
            first = s[len('return '):]
            # gather the whole statement (to the ';' that closes it)
            depth = 0
            j = i
            stmt = ''
            while j < n:
                seg = lines[j] if j > i else first
                stmt += seg + NL
                depth += (seg.count('(') + seg.count('{') + seg.count('[')
                          - seg.count(')') - seg.count('}') - seg.count(']'))
                if depth <= 0 and lines[j].rstrip().endswith(';'):
                    break
                j += 1
            expr = stmt.strip()
            assert expr.endswith(';'), expr[:120]
            expr = expr[:-1].strip()
            m = re.match(r'^Box::pin\(\s*async move \{(.*)\}\s*,?\s*\)$', expr, re.S)
            if m:
                inner = m.group(1).strip(NL)
                if '.await' in inner:
                    raise SystemExit('unhoisted async short-circuit: ' + s[:80])
                res.append(indent + 'return Flow::Respond(' + inner + ');')
            elif NL in expr:
                res.append(indent + 'return Flow::Respond(')
                for il in expr.split(NL):
                    t = il.strip()
                    res.append((indent + '    ' + t) if t else '')
                res.append(indent + ');')
            else:
                res.append(indent + 'return Flow::Respond(' + expr + ');')
            i = j + 1
            continue
        res.append(l)
        i += 1
    return res


STAGE_FILE = {'match': 'matcher'}


def apply_rules(text, rules):
    # rules run over the JOINED span text: a `&name` at end-of-line can have
    # its `.` consumer on the next line (rustfmt-wrapped chains).
    if not rules:
        return text
    for rx, repl in rules:
        text = rx.sub(repl, text)
    return text


def stage_borrow_rules(stage):
    # Same needless_borrow derivation as build_phase_split.borrow_rules, over
    # the stage re-binds: every `&name` not followed by `.` drops the `&` when
    # the re-bind already made `name` a reference. Arc-wrapped re-binds
    # (depsarc) and owned kinds keep their `&` (an `Arc` is not `&T`).
    rules = []
    for name, how in REBINDS.get(stage, {}).items():
        if how in ('ref', 'depsref', 'ctxreq', 'optref') or (stage == 'route' and name == 'req_owned'):
            rules.append((re.compile(r'(?<![\w&])&%s\b(?!\s*[.\[])' % re.escape(name)), name))
            rules.append((re.compile(r'(?<![\w&])(%s): %s(?=\s*[,)])' % (re.escape(name),
                                                                          re.escape(name))),
                          r'\1'))
    return rules


def emit_stage(lines, stage):
    lo, hi = SPANS[stage]
    body = transform_span(lines, lo, hi, flow_only=stage in FLOW_STAGES)
    if stage in ASYNC_SPANS:
        body = [l[4:] if l.startswith('    ') else (l if l.strip() else '')
                for l in body]
    body = apply_rules(NL.join(body), stage_borrow_rules(stage)).split(NL)
    w = []
    w.append('//! Moved verbatim from dispatch/entry.rs by scripts/build_stage_split.py')
    w.append('//! (V2-A01 phase 3).')
    w.append('//!')
    for dl in STAGE_DOCS[stage].split(NL):
        w.append('//! ' + dl)
    w.append('')
    w.append('use crate::*;')
    if stage in FLOW_STAGES:
        w.append('use oxide_fw_dispatch::flow::Flow;')
    w.append('')
    w.append('// >>> stage-wrapper (not part of the moved body; split_selfcheck --stages strips this block)')
    flow_ret = 'Flow' if stage in FLOW_STAGES else 'oxide_fw_server::OxideResponse'
    sig = ("pub(crate) async fn stage_%s(\n    d: &DispatchDeps,\n"
           "    cx: &mut RequestCtx,\n) -> %s {" % (stage, flow_ret))
    w.append(sig)
    for name, how in REBINDS[stage].items():
        rl = rebind_line(name, how)
        if rl:
            w.append(rl)
    w.append('// <<< stage-wrapper')
    w.extend(body)
    if stage in HANDOFFS and HANDOFFS[stage]:
        w.append('// >>> stage-wrapper (not part of the moved body; split_selfcheck --stages strips this block)')
        for field, kind in HANDOFFS[stage]:
            if kind == 'Some':
                w.append('    cx.%s = Some(%s);' % (field, field))
            elif kind == 'bare':
                w.append('    cx.%s = %s;' % (field, field))
            else:
                raise ValueError(kind)
        if stage in FLOW_STAGES:
            w.append('    Flow::Continue')
        w.append('// <<< stage-wrapper')
    elif stage in FLOW_STAGES:
        w.append('// >>> stage-wrapper (not part of the moved body; split_selfcheck --stages strips this block)')
        w.append('    Flow::Continue')
        w.append('// <<< stage-wrapper')
    w.append('}')
    fp = os.path.join(SRC, 'stages', STAGE_FILE.get(stage, stage) + '.rs')
    with open(fp, 'w') as f:
        f.write(NL.join(w) + NL)
    return len(w)

# RequestCtx field table: (name, type, initial value in new()).
CTX_FIELDS = [
    ('req', 'oxide_fw_server::OxideRequest', None),
    ('tree', 'std::sync::Arc<oxide_fw_router::RouteTree>', None),
    ('started', 'std::time::Instant', 'std::time::Instant::now()'),
    ('boot_build_id', 'Option<String>', 'None'),
    ('rewrite_hop', 'u32', '0'),
    ('normalized', 'String', 'String::new()'),
    ('raw', 'String', 'String::new()'),
    ('req_path', 'String', 'String::new()'),
    ('resolved_locale', 'String', 'String::new()'),
    ('is_data_request', 'bool', 'false'),
    ('matched', 'Option<oxide_fw_router::MatchResult>', 'None'),
    ('status', 'u16', '0'),
    ('label', 'String', 'String::new()'),
    ('loader_runner', 'Option<std::sync::Arc<dyn oxide_fw_server::LoaderRunner>>', 'None'),
    ('page_renderer', 'Option<std::sync::Arc<dyn oxide_fw_server::PageRenderer>>', 'None'),
    ('module_path', 'Option<std::path::PathBuf>', 'None'),
    ('route_id', 'Option<String>', 'None'),
    ('params', 'Option<std::collections::BTreeMap<String, String>>', 'None'),
    ('has_server_module', 'bool', 'false'),
    ('render_mode', 'Option<String>', 'None'),
    ('needs_parent', 'bool', 'false'),
    ('is_live', 'bool', 'false'),
    ('endpoint_module_path', 'Option<std::path::PathBuf>', 'None'),
    ('middleware_module_paths', 'Vec<std::path::PathBuf>', 'Vec::new()'),
    ('endpoint_params_json', 'Option<serde_json::Map<String, serde_json::Value>>', 'None'),
    ('endpoint_body', 'bytes::Bytes', 'bytes::Bytes::new()'),
    ('request_nonce', 'String', 'String::new()'),
    ('unmatched_route_id', 'Option<String>', 'None'),
    ('unmatched_layout_ids', 'Vec<String>', 'Vec::new()'),
    ('error_route_id', 'Option<String>', 'None'),
    ('route_id_for_render', 'Option<String>', 'None'),
    ('layout_ids', 'Vec<String>', 'Vec::new()'),
    ('layout_module_paths', 'Vec<(String, std::path::PathBuf)>', 'Vec::new()'),
    ('action_name', 'Option<String>', 'None'),
    ('action_dispatcher', 'Option<std::sync::Arc<dyn oxide_fw_server::ActionDispatcher>>', 'None'),
    ('endpoint_for_dispatch', 'Option<std::sync::Arc<dyn oxide_fw_server::EndpointDispatcher>>', 'None'),
    ('action_params', 'serde_json::Map<String, serde_json::Value>', 'serde_json::Map::new()'),
    ('action_route', 'String', 'String::new()'),
    ('req_recording', 'Option<crate::record_wire::RequestRecording>', 'None'),
    ('waterfall', 'Option<oxide_fw_dev::WaterfallRecorder>', 'None'),
    ('brownout_ceiling', 'Option<u64>', 'None'),
    ('l0_eligible', 'bool', 'false'),
    ('l0_route', 'String', 'String::new()'),
    ('l0_params', 'String', 'String::new()'),
    ('l0_vary', 'String', 'String::new()'),
    ('l0_path', 'String', 'String::new()'),
    ('flight_key', 'String', 'String::new()'),
    ('middleware_locals', 'serde_json::Value', 'serde_json::Value::Null'),
    ('middleware_headers', 'Vec<(String, String)>', 'Vec::new()'),
    ('middleware_ms', 'f64', '0.0'),
    ('session_before', 'Option<serde_json::Value>', 'None'),
    ('action_form', 'Option<serde_json::Value>', 'None'),
    ('flight_owner', 'Option<oxide_fw_cache::FlightOwnerGuard>', 'None'),
    ('data', 'Option<serde_json::Value>', 'None'),
    ('page_settles', 'Option<tokio::sync::mpsc::Receiver<serde_json::Value>>', 'None'),
    ('route_meta', 'Option<serde_json::Value>', 'None'),
    ('loader_set_cookies', 'Option<Vec<String>>', 'None'),
    ('loader_headers', 'Option<Vec<(String, String)>>', 'None'),
    ('deferred_values', 'std::collections::BTreeMap<String, serde_json::Value>',
     'std::collections::BTreeMap::new()'),
    ('stage_load_start', 'std::time::Instant', 'std::time::Instant::now()'),
    ('boot_script_url_for_render', 'Option<String>', 'None'),
    ('boot_script_integrity_for_render', 'Option<String>', 'None'),
]


def emit_ctx():
    w = []
    w.append('//! V2-A01 phase 3: the per-request stage context (spec WP-A01 "Step 3").')
    w.append('//!')
    w.append('//! The 1,400-line `build_dispatch` closure carried its per-request state as')
    w.append('//! ~60 locals; those locals are the [`RequestCtx`] fields here. The command')
    w.append('//! request and the loaded route tree arrive through [`RequestCtx::new`]; the')
    w.append('//! stage that creates a value hands it to the context at its end, and a later')
    w.append('//! stage re-binds it (`&cx.field`, `&mut cx.field`, `clone`, or `take` for the')
    w.append('//! four fields a stage MOVES out of: `params`, `data`, `action_form`,')
    w.append('//! `page_settles`) so the moved body text stays untouched.')
    w.append('')
    w.append('')
    w.append('/// The boot handles a demoted generation swaps in per request (the B-04')
    w.append('/// tuple), routed by stage_route through the serving pools.')
    w.append('pub(crate) type RoutedGenerationHandles = (')
    w.append('    Option<std::sync::Arc<dyn oxide_fw_server::LoaderRunner>>,')
    w.append('    Option<std::sync::Arc<dyn oxide_fw_server::PageRenderer>>,')
    w.append('    Option<std::sync::Arc<dyn oxide_fw_jshost::Pool>>,')
    w.append('    std::path::PathBuf,')
    w.append(');')
    w.append('')
    w.append('/// The per-request mutable state threaded through the dispatch stages.')
    w.append('pub(crate) struct RequestCtx {')
    for name, ty, _init in CTX_FIELDS:
        w.append('    pub(crate) %s: %s,' % (name, ty))
    w.append('}')
    w.append('')
    w.append('impl RequestCtx {')
    w.append('    /// The stage entry state: the request itself plus the route tree the')
    w.append('    /// prelude loaded through the swappable slot (the tree read MUST stay')
    w.append('    /// sync-side: an `Fn` dispatch closure cannot hold the read guard across')
    w.append('    /// the stage awaits).')
    w.append('    pub(crate) fn new(')
    w.append('        req: oxide_fw_server::OxideRequest,')
    w.append('        tree: std::sync::Arc<oxide_fw_router::RouteTree>,')
    w.append('    ) -> Self {')
    w.append('        Self {')
    w.append('            req,')
    w.append('            tree,')
    for name, _ty, init in CTX_FIELDS:
        if init is not None:
            w.append('            %s: %s,' % (name, init))
    w.append('        }')
    w.append('    }')
    w.append('}')
    fp = os.path.join(SRC, 'ctx.rs')
    with open(fp, 'w') as f:
        f.write(NL.join(w) + NL)
    return len(w)


def emit_deps(head_lines):
    # DispatchDeps = the DispatchArgs field block (HEAD 151-213) verbatim with
    # the struct renamed, plus the three request-path runtime handles the old
    # setup block built per boot.
    w = []
    w.append('//! V2-A01 phase 3: the dispatch deps (spec WP-A01 "Step 3" sketch).')
    w.append('//!')
    w.append('//! The sketch\'s 18-field `DispatchDeps` — extended with the fields the')
    w.append('//! pipeline grew since the sketch (F07/F10/F12/F15/A04 handles) and the three')
    w.append('//! request-path runtime handles the old `build_dispatch_swappable` setup')
    w.append('//! block built per boot (the PERF-04 single-flight map, the §8.5')
    w.append('//! self-redispatch lock and the F07 stream-id counter). The stage functions')
    w.append('//! take `&DispatchDeps`; the dispatch closure captures one `Arc<DispatchDeps>`,')
    w.append('//! which is what keeps it an `Fn`.')
    w.append('')
    w.append('use crate::*;')
    w.append('')
    w.append('/// PERF-04: the single-flight map (one per process, shared across every')
    w.append('/// dispatch call).')
    w.append('pub(crate) type FlightMap = dashmap::DashMap<')
    w.append('    String,')
    w.append('    std::sync::Arc<tokio::sync::broadcast::Sender<std::sync::Arc<oxide_fw_cache::Entry>>>,')
    w.append('>;')
    w.append('')
    w.append('/// §8.5: the self-redispatch slot the middleware `{ rewrite }` arm reads.')
    w.append('pub(crate) type SelfDispatch = std::sync::OnceLock<')
    w.append('    std::sync::Arc<dyn Fn(oxide_fw_server::OxideRequest) -> BoxedResponse + Send + Sync>,')
    w.append('>;')
    w.append('')
    w.append('/// The boot-time dispatch dependencies shared by every request.')
    w.append('pub(crate) struct DispatchDeps {')
    for i in range(152, 213):
        w.append(head_lines[i - 1])
    w.append('    /// PERF-04: single-flight coalescing. Concurrent misses for the same L0')
    w.append('    /// key run one render; the rest await the owner\'s entry.')
    w.append('    pub flight_map: std::sync::Arc<FlightMap>,')
    w.append('    /// §8.5: published once after the wrapper exists; a middleware')
    w.append('    /// `{ rewrite }` re-enters through it (the hop header bounds the loop).')
    w.append('    pub self_dispatch: std::sync::Arc<SelfDispatch>,')
    w.append('    /// V2-F07: the per-process stream-id counter the endpoint tee mints')
    w.append('    /// `stream_id(request_id, counter)` values with.')
    w.append('    pub stream_counter: std::sync::Arc<std::sync::atomic::AtomicU64>,')
    w.append('}')
    fp = os.path.join(SRC, 'deps.rs')
    with open(fp, 'w') as f:
        f.write(NL.join(w) + NL)
    return len(w)


def emit_stages_mod():
    w = []
    w.append('//! V2-A01 phase 3: the dispatch stage table (spec WP-A01 "Step 3").')
    w.append('//!')
    w.append('//! The closure became: build [`RequestCtx`], then run the stages in order —')
    w.append('//! `stage_prelude -> stage_match -> stage_route -> stage_middleware ->')
    w.append('//! stage_data_route -> stage_action -> stage_l0_lookup -> stage_loaders ->')
    w.append('//! stage_endpoint -> stage_render`. The first nine short-circuit through')
    w.append('//! [`oxide_fw_dispatch::Flow`]; the last (the sketch\'s `stage_render` +')
    w.append('//! `stage_finish` pair) produces the final response.')
    w.append('')
    w.append('use crate::*;')
    w.append('')
    for s in STAGE_ORDER:
        m = STAGE_FILE.get(s, s)
        w.append('mod %s;' % m)
        w.append('pub(crate) use %s::*;' % m)
    w.append('')
    w.append('/// The stage table, in call order (the phase-3 runner the sketch describes:')
    w.append('/// `for stage in STAGES { if let Flow::Respond(r) = stage(d, &mut cx).await {')
    w.append('/// return r } }` — spelled out so the compiler checks the order and every')
    w.append('/// stage signature).')
    w.append('pub(crate) async fn run_stages(')
    w.append('    d: &DispatchDeps,')
    w.append('    cx: &mut RequestCtx,')
    w.append(') -> oxide_fw_server::OxideResponse {')
    for s in STAGE_ORDER[:-1]:
        w.append('    if let oxide_fw_dispatch::flow::Flow::Respond(r) = stage_%s(d, cx).await {' % s)
        w.append('        return r;')
        w.append('    }')
    w.append('    stage_render(d, cx).await')
    w.append('}')
    fp = os.path.join(SRC, 'stages', 'mod.rs')
    with open(fp, 'w') as f:
        f.write(NL.join(w) + NL)
    return len(w)


def patch_literal(lines, marker):
    """Append the three runtime-handle fields to a `DispatchDeps {` literal that
    ends with `serving_pools: None,` + `    })` (the build_dispatch helper and
    the serve boot construct the same shape)."""
    out = list(lines)
    if not any(marker in l for l in out):
        return out  # already patched (idempotent rerun)
    if any('flight_map: std::sync::Arc::new(dashmap::DashMap::new())' in l for l in out):
        return out  # runtime fields already present
    idx = next(i for i, l in enumerate(out) if marker in l)
    end = next(i for i in range(idx, len(out))
               if out[i].strip().startswith('serving_pools:')) + 1
    insert = [
        '        flight_map: std::sync::Arc::new(dashmap::DashMap::new()),',
        '        self_dispatch: std::sync::Arc::new(std::sync::OnceLock::new()),',
        '        stream_counter: std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0)),',
    ]
    return out[:end] + insert + out[end:]


def main():
    import subprocess
    repo = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
    head = subprocess.run(
        ['git', '-C', repo, 'show', 'HEAD:crates/cli/src/dispatch/entry.rs'],
        capture_output=True, text=True, check=True).stdout
    lines = head.split(NL)
    # boundary assertions against the HEAD monolith
    assert lines[150].startswith('pub(crate) struct DispatchArgs {'), lines[150]
    assert lines[212].strip() == '}', lines[212]
    assert lines[310].strip() == 'let inner_dispatch: std::sync::Arc<', lines[310]
    assert lines[2770].strip() == '})', lines[2770]
    assert lines[2771].strip() == '});', lines[2771]
    assert lines[2833].strip().startswith('let _ = self_dispatch.set('), lines[2833]

    os.makedirs(os.path.join(SRC, 'stages'), exist_ok=True)
    written = {}
    written['deps.rs'] = emit_deps(lines)
    written['ctx.rs'] = emit_ctx()
    for s in STAGE_ORDER:
        written['stages/%s.rs' % s] = emit_stage(lines, s)
    written['stages/mod.rs'] = emit_stages_mod()

    # --- entry.rs: keep the test helper + the two small helpers, replace the
    # DispatchArgs struct + the monolithic closure, keep the A04 wrapper.
    new = []
    for i in range(1, 150):
        new.append(lines[i - 1])
    new = patch_literal(new, 'build_dispatch_swappable(DispatchArgs {')
    # the literal's struct name changes with the move (DispatchArgs -> DispatchDeps)
    new = [l.replace('build_dispatch_swappable(DispatchArgs {',
                     'build_dispatch_swappable(DispatchDeps {') for l in new]
    new.append('')
    new.append('// >>> stage-wrapper (not part of the moved body; split_selfcheck --stages strips this block)')
    new.append('/// The framework dispatch itself: the stage runner over one shared')
    new.append('/// `Arc<DispatchDeps>` (the A04 wrapper below wraps it). The old')
    w2 = []
    w2.append('/// ~30-capture closure became the stage table — the bodies moved verbatim')
    w2.append('/// into `stages/` (see scripts/build_stage_split.py + split_selfcheck --stages).')
    w2.append('pub(crate) fn build_dispatch_swappable(')
    w2.append('    args: DispatchDeps,')
    w2.append(') -> std::sync::Arc<dyn Fn(oxide_fw_server::OxideRequest) -> BoxedResponse + Send + Sync> {')
    w2.append('    let deps = std::sync::Arc::new(args);')
    w2.append('    let inner_dispatch: std::sync::Arc<')
    w2.append('        dyn Fn(oxide_fw_server::OxideRequest) -> BoxedResponse + Send + Sync,')
    w2.append('    > = {')
    w2.append('        let deps = std::sync::Arc::clone(&deps);')
    w2.append('        std::sync::Arc::new(move |req: oxide_fw_server::OxideRequest| {')
    w2.append('            let deps = std::sync::Arc::clone(&deps);')
    w2.append('            Box::pin(async move {')
    w2.append('                let mut cx = RequestCtx::new(req, deps.swappable.load());')
    w2.append('                stages::run_stages(&deps, &mut cx).await')
    w2.append('            })')
    w2.append('        })')
    w2.append('    };')
    w2.append('    // The A04 closure moves the deps Arc; the publish below goes through')
    w2.append('    // this clone (the same OnceLock).')
    w2.append('    let deps_for_publish = std::sync::Arc::clone(&deps);')
    w2.append('// <<< stage-wrapper')
    new.extend(w2)
    # the A04 wrapper + publish (HEAD 2773-2836), with the two dep-pointer edits
    for i in range(2773, 2837):
        l = lines[i - 1]
        if i == 2795:
            l = l.replace('generations.as_ref()', 'deps.generations.as_ref()')
        if i == 2834:
            l = l.replace('let _ = self_dispatch.set(',
                          'let _ = deps_for_publish.self_dispatch.set(')
        new.append(l)
    # the closing brace of build_dispatch_swappable was line 2836 `}` — kept above.
    with open(os.path.join(SRC, 'entry.rs'), 'w') as f:
        f.write(NL.join(new) + NL)
    written['entry.rs'] = len(new)

    # --- dispatch/mod.rs wiring
    mod = []
    mod.append('//! V2-A01 module root.')
    mod.append('')
    mod.append('mod entry;')
    mod.append('pub(crate) use entry::*;')
    mod.append('// V2-A01 phase 3: the stage protocol over DispatchDeps + RequestCtx.')
    mod.append('mod deps;')
    mod.append('pub(crate) use deps::*;')
    mod.append('mod ctx;')
    mod.append('pub(crate) use ctx::*;')
    mod.append('pub(crate) mod stages;')
    with open(os.path.join(SRC, 'mod.rs'), 'w') as f:
        f.write(NL.join(mod) + NL)
    written['mod.rs'] = len(mod)

    # --- serve/start.rs boot literal
    sp = os.path.join(repo, 'crates', 'cli', 'src', 'serve', 'start.rs')
    st = open(sp).read().split(NL)
    st2 = patch_literal(st, 'build_dispatch_swappable(DispatchArgs {')
    st2 = [l.replace('build_dispatch_swappable(DispatchArgs {',
                     'build_dispatch_swappable(DispatchDeps {') for l in st2]
    with open(sp, 'w') as f:
        f.write(NL.join(st2) + NL)
    written['serve/start.rs'] = len(st2)
    print('wrote:', written)


if __name__ == '__main__':
    main()
