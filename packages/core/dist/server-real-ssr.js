import { beginBoundaryCapture, boundaryContent, drainBoundaries, endBoundaryCapture } from "./server-boundary.js";
import { resolveDeferredPlaceholders } from "./server-deferred.js";
// src/server/real-ssr.ts
const TEXT_ESCAPES = {
	"&": "&amp;",
	"<": "&lt;",
	">": "&gt;"
};
const ATTR_ESCAPES = {
	"&": "&amp;",
	"\"": "&quot;",
	"<": "&lt;"
};
const escText = (s) => s.replace(/[&<>]/g, (c) => TEXT_ESCAPES[c]);
const escAttr = (s) => s.replace(/[&"<]/g, (c) => ATTR_ESCAPES[c]);
/** Render a neutral HeadTags model to the strings that go inside <head>. */
function emitHead(head) {
	const parts = [];
	if (head.title !== void 0) {
		const t = head.titleTemplate && head.titleTemplate.includes("%s") ? head.titleTemplate.replace("%s", head.title) : head.title;
		parts.push(`<title>${escText(t)}</title>`);
	}
	if (head.base) parts.push(`<base href="${escAttr(head.base.href)}"${head.base.target ? ` target="${escAttr(head.base.target)}"` : ""}>`);
	for (const m of head.meta) {
		const attrs = Object.entries(m).map(([k, v]) => ` ${k}="${escAttr(v)}"`).join("");
		parts.push(`<meta${attrs}>`);
	}
	for (const l of head.link) {
		const attrs = Object.entries(l).map(([k, v]) => ` ${k}="${escAttr(v)}"`).join("");
		parts.push(`<link${attrs}>`);
	}
	for (const s of head.style) parts.push(`<style${s.nonce ? ` nonce="${escAttr(s.nonce)}"` : ""}>${s.content}</style>`);
	for (const s of head.script) {
		const attrs = [
			s.src ? ` src="${escAttr(s.src)}"` : "",
			s.type ? ` type="${escAttr(s.type)}"` : "",
			s.nonce ? ` nonce="${escAttr(s.nonce)}"` : "",
			s.defer ? " defer" : ""
		].join("");
		parts.push(`<script${attrs}>${s.content ? escText(s.content) : ""}<\/script>`);
	}
	for (const entry of head.jsonLd ?? []) {
		const body = typeof entry === "string" ? entry : JSON.stringify(entry);
		parts.push(`<script type="application/ld+json">${escText(body)}<\/script>`);
	}
	return parts.join("");
}
function streamOf(html) {
	const enc = new TextEncoder();
	return new ReadableStream({ start(c) {
		c.enqueue(enc.encode(html));
		c.close();
	} });
}
/**
* §7.1: render one registered boundary as the `[data-ox-c]` + swapper chunk.
*
* Rendered standalone — a fresh render of the resolved content, not a
* continuation of the shell. That is what lets the shell stream before any
* boundary resolves.
*/
async function renderBoundary(b, cfg, input) {
	let html;
	try {
		html = await cfg.renderBody(b.Component, b.props);
	} catch (err) {
		const msg = err instanceof Error ? err.message : String(err);
		html = `<div class='ox-error' data-ox-error='OX-500'>OX-500: ${String(msg).replace(/[&<>]/g, (c) => ({
			"&": "&amp;",
			"<": "&lt;",
			">": "&gt;"
		})[c])}</div>`;
	}
	return boundaryContent(b.id, html, input.nonce ?? "");
}
function makeRealAdapter(cfg) {
	const strategy = cfg.opts.ssr ?? "auto";
	const unwrapModuleDefault = (v, id) => {
		if (v === null || typeof v !== "object") return v;
		if (typeof v === "function") return v;
		const tag = Reflect.get(v, Symbol.toStringTag);
		const strTag = Object.prototype.toString.call(v);
		const hasDefault = "default" in v;
		const defaultVal = hasDefault ? v.default : void 0;
		const defaultType = typeof defaultVal;
		if (tag === "Module" || strTag === "[object Module]") return defaultVal;
		if (hasDefault && defaultType === "function") return defaultVal;
		if (Reflect.get(v, "$$typeof") !== void 0) return v;
		return v;
	};
	const resolveComponent = (input, id) => {
		const r = input.resolve;
		if (typeof r === "function") return unwrapModuleDefault(r(id), id);
		return unwrapModuleDefault(cfg.opts.componentFor?.(id), id);
	};
	const missing = (kind, id) => {
		if (id.startsWith("__oxide_")) return;
		console.error(`[oxide] ${cfg.name}: no component resolved for ${kind} \`${id}\``);
	};
	return {
		contract: 1,
		name: cfg.name,
		extensions: cfg.extensions,
		...cfg.jsxImportSource ? { jsxImportSource: cfg.jsxImportSource } : {},
		capabilities: cfg.capabilities,
		plugins: cfg.plugins,
		head(ctx) {
			const synthetic = {
				route: ctx.route,
				url: ctx.url,
				params: ctx.params,
				data: ctx.data,
				head: {
					meta: [],
					link: [],
					style: [],
					script: []
				},
				locals: ctx.locals
			};
			return cfg.buildHead(synthetic);
		},
		render(input) {
			const Component = resolveComponent(input, input.route);
			const safeComment = (s) => String(s).replace(/-->/g, "--\\>").replace(/</g, "&lt;");
			if (Component === void 0 || Component === null) {
				if (strategy === "real") {
					missing("route", input.route);
					return streamOf(`<!--oxide-status:500--><div class='ox-error' data-ox-error='OX-500'>OX-500: no component resolved for route ${escText(input.route)}</div>`);
				}
				return cfg.irRender(input);
			}
			const enc = new TextEncoder();
			const it = (async function* () {
				if (!await cfg.isAvailable()) {
					if (strategy === "real") {
						const why = safeComment(cfg.lastError?.() ?? "isAvailable=false, no recorded error");
						const head = cfg.buildHead(input);
						yield enc.encode(`<!doctype html>\n<html lang="${input.locale && /^[A-Za-z0-9_-]+$/.test(input.locale) ? input.locale : "en"}"><head>${emitHead(head)}</head>`);
						yield enc.encode(`<body><div${cfg.rootAttrs}>`);
						yield enc.encode(`<div class='ox-error' data-ox-error='OX-500'>OX-500: ${escText(cfg.name)} SSR unavailable: ${escText(why)}</div>`);
						yield enc.encode(`</div></body></html>`);
						return;
					}
					for await (const c of cfg.irRender(input)) yield c;
					return;
				}
				const head = cfg.buildHead(input);
				const layoutIds = Array.isArray(input.layouts) ? input.layouts : [];
				const layoutParts = [];
				for (const id of layoutIds) {
					const L = resolveComponent(input, id);
					if (L === void 0 || L === null) {
						if (strategy === "real") {
							missing("layout", id);
							layoutParts.push({
								id,
								Component: void 0
							});
						}
						continue;
					}
					layoutParts.push({
						id,
						Component: L
					});
				}
				if (layoutParts.length === 0) {
					const single = cfg.opts.layoutFor?.(input.route);
					if (single !== void 0 && single !== null) layoutParts.push({
						id: `layoutFor(${input.route})`,
						Component: single
					});
				}
				const headWarnings = [];
				const rawData = structuredClone(input.data);
				const data = resolveDeferredPlaceholders(structuredClone(input.data), input.deferred);
				const prevCapture = beginBoundaryCapture();
				const props = {
					route: input.route,
					url: input.url,
					params: input.params,
					data,
					rawData,
					deferred: input.deferred
				};
				try {
					let descriptor = {
						Component,
						props
					};
					for (const layer of [...layoutParts].reverse()) {
						if (layer.Component === void 0) {
							headWarnings.push(`<!-- oxide: layout ${safeComment(layer.id)} declared but the resolver returned undefined; skipped -->`);
							continue;
						}
						descriptor = {
							Component: layer.Component,
							props: {
								...props,
								children: descriptor
							}
						};
					}
					yield enc.encode(`<!doctype html>\n<html lang="${input.locale && /^[A-Za-z0-9_-]+$/.test(input.locale) ? input.locale : "en"}"><head>${emitHead(head)}</head>`);
					if (headWarnings.length > 0) yield enc.encode(`<body>${headWarnings.join("")}<div${cfg.rootAttrs}>`);
					else yield enc.encode(`<body><div${cfg.rootAttrs}>`);
					if (typeof cfg.renderStream === "function") {
						const reader = (await cfg.renderStream(descriptor.Component, descriptor.props)).getReader();
						for (;;) {
							const { value, done } = await reader.read();
							if (done) break;
							if (value && value.length > 0) yield value;
						}
					} else {
						const body = await cfg.renderBody(descriptor.Component, descriptor.props);
						yield enc.encode(body);
					}
					yield enc.encode("</div></body></html>");
					const registered = drainBoundaries();
					endBoundaryCapture(prevCapture);
					for (const b of registered) yield enc.encode(await renderBoundary(b, cfg, input));
				} catch (err) {
					const msg = err instanceof Error ? err.message : String(err);
					if (err instanceof Error && err.stack) console.error("[oxide:render-fail]", err.stack);
					const errId = input.errorRoute;
					if (typeof errId === "string" && errId.length > 0) {
						const ErrComp = resolveComponent(input, errId);
						if (ErrComp !== void 0 && ErrComp !== null) try {
							const errProps = {
								...props,
								error: {
									message: msg,
									route: input.route,
									status: 500
								}
							};
							const body = await cfg.renderBody(ErrComp, errProps);
							yield enc.encode(`<!--oxide-status:500--><!doctype html>\n<html lang="${input.locale && /^[A-Za-z0-9_-]+$/.test(input.locale) ? input.locale : "en"}"><head>${emitHead(head)}</head>`);
							yield enc.encode(`<body><div${cfg.rootAttrs}>${body}</div></body></html>`);
							return;
						} catch {}
					}
					yield enc.encode(`<!--oxide-status:500--><!doctype html>\n<html lang="${input.locale && /^[A-Za-z0-9_-]+$/.test(input.locale) ? input.locale : "en"}"><head>${emitHead(head)}</head>`);
					yield enc.encode(`<body><div${cfg.rootAttrs}>`);
					yield enc.encode(`<div class='ox-error' data-ox-error='OX-500'>OX-500: ${escText(msg)}</div>`);
					yield enc.encode(`</div></body></html>`);
				}
			})();
			return new ReadableStream({ async pull(c) {
				try {
					const { value, done } = await it.next();
					if (done) c.close();
					else c.enqueue(value);
				} catch (err) {
					c.error(err);
				}
			} });
		},
		hydrate: cfg.hydrate,
		...cfg.hydrateIsland ? { hydrateIsland: cfg.hydrateIsland } : {},
		mount: cfg.mount,
		update: cfg.update
	};
}
export { emitHead, makeRealAdapter, resolveDeferredPlaceholders };

//# sourceMappingURL=server-real-ssr.js.map