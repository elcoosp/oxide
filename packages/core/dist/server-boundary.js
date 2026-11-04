// src/server/boundary.ts
function slot() {
	return globalThis.__oxide_boundary_registry;
}
/**
* Start capturing boundaries. Returns the previous slot so a nested render
* (an island, an error boundary) restores it instead of clobbering the
* outer capture.
*
* Safe as a process-global because the worker serialises calls into one
* module instance — the same assumption `__oxide_fn_registry` makes.
*/
function beginBoundaryCapture() {
	const prev = slot();
	globalThis.__oxide_boundary_registry = [];
	return prev;
}
/** Restore the slot captured by `beginBoundaryCapture`. */
function endBoundaryCapture(prev) {
	if (prev === void 0) delete globalThis.__oxide_boundary_registry;
	else globalThis.__oxide_boundary_registry = prev;
}
/** Take everything captured so far. Leaves the slot empty for the next batch. */
function drainBoundaries() {
	const cur = slot();
	if (!cur) return [];
	return cur.splice(0, cur.length);
}
/** Called by `<Defer>` while the shell is rendering. A no-op outside a
*  capture: a component rendered standalone still emits its placeholder. */
function registerBoundary(entry) {
	const cur = slot();
	if (!cur) return;
	if (cur.some((e) => e.id === entry.id)) return;
	cur.push(entry);
}
/** The `{ __deferred: id }` placeholder a loader's data carries, or null. */
function deferredIdOf(value) {
	if (value === null || typeof value !== "object") return null;
	const v = value;
	if (typeof v.__deferred !== "number") return null;
	if (Object.keys(value).length !== 1) return null;
	return v.__deferred;
}
const BOUNDARY_FALLBACK_ATTR = "data-ox-b";
const BOUNDARY_CONTENT_ATTR = "data-ox-c";
const TEXT_ESCAPES = {
	"&": "&amp;",
	"<": "&lt;",
	">": "&gt;"
};
const ATTR_ESCAPES = {
	"&": "&amp;",
	"\"": "&quot;",
	"<": "&lt;",
	">": "&gt;",
	"'": "&#39;"
};
function escapeText(s) {
	return s.replace(/[&<>]/g, (c) => TEXT_ESCAPES[c]);
}
function escapeAttr(s) {
	return s.replace(/[&"'<>]/g, (c) => ATTR_ESCAPES[c]);
}
function escapeScript(s) {
	return s.replace(/<\/(script)/gi, "<\\/$1").replace(/<!--/g, "<\\!--");
}
/** Attribute-only selectors so one swapper drives both host shapes: the IR
*  renderer's `<template>` and a framework's `<ox-defer>`. */
const SWAPPER_SRC = "function(id){var t=document.querySelector(\"[data-ox-c='\"+id+\"']\"),p=document.querySelector(\"[data-ox-b='\"+id+\"']\");if(t&&p){var d=document.createElement(\"div\");d.innerHTML=t.innerHTML;p.replaceWith.apply(p,d.childNodes);document.dispatchEvent(new CustomEvent(\"ox:swap\",{detail:{id:id,value:t.innerHTML}}))}}";
function boundaryFallback(id, fallbackHtml) {
	return `<template ${BOUNDARY_FALLBACK_ATTR}='${id}'>${fallbackHtml}</template>`;
}
function boundaryContent(id, html, nonce = "") {
	const n = nonce ? ` nonce='${escapeAttr(nonce)}'` : "";
	return `<template ${BOUNDARY_CONTENT_ATTR}='${id}'>${html}</template><script${n}>(${escapeScript(SWAPPER_SRC)})('${id}')<\/script>`;
}
export { BOUNDARY_CONTENT_ATTR, BOUNDARY_FALLBACK_ATTR, SWAPPER_SRC, beginBoundaryCapture, boundaryContent, boundaryFallback, deferredIdOf, drainBoundaries, endBoundaryCapture, escapeAttr, escapeText, registerBoundary };

//# sourceMappingURL=server-boundary.js.map