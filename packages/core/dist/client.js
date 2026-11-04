import { a as decodeOdf, i as OdfError, n as MAX_DEPTH, o as decodeOdfToJson, r as MAX_REFS, t as MAX_BYTES } from "./odf-decode-D1PLHGuy.js";
import { componentFor, getRouteComponentSource, isParamSegment, pageComponentExport, routeIdFor, setRouteComponentSource } from "./client-route-manifest.js";
import { createRouter } from "./client-router.js";
import { decodeIslandProps, hydrateIsland, hydrateIslands, parseStrategy } from "./client-islands.js";
import { DeferredClient, attachDeferGlue, wireDeferreds } from "./client-defer-glue.js";
import { enhance } from "./client-enhance.js";
import { n as scheduleScript, t as loadScripts } from "./scripts-BUPfxzLn.js";
import { loadMessages, setMessageLocales, t } from "./client-messages.js";
import { t as reportWebVitals } from "./vitals-L5C-0ePE.js";
// src/client/search.ts
/**
* Tokenize by lowercasing and splitting on any non-alphanumeric run.
* Drops single-character tokens (`a`, `of`) — a 1-char query rarely
* discriminates and inflates the score for common words.
*/
function tokenize(s) {
	return s.toLowerCase().split(/[^a-z0-9]+/).filter((t) => t.length >= 2);
}
/**
* Prefix-match a query token against a set of index tokens. A query token
* matches when it is a prefix of any index token (`install` matches
* `installation`; `getting` matches `getting`). Substring matching
* (`stal` matching `installation`) is deliberately not supported — it
* produces so many false positives for short queries that a result list
* becomes noise.
*/
function matchesAny(qt, tokens) {
	for (const t of tokens) if (t.startsWith(qt)) return true;
	return false;
}
/**
* Rank index entries against a free-text query. Multi-token queries are
* AND-scored (all tokens contribute, each against the entry) — the
* result list is filtered to entries with score > 0, so a token that
* matches nothing in an entry adds nothing rather than excluding it.
* That is the behaviour a search box wants: partial matches still rank,
* just lower.
*
* Per query token, the score is:
*   +3 if the token prefix-matches any title token
*   +2 if it prefix-matches any heading token
*   +1 if it prefix-matches any excerpt token
*
* Prefix matching is what makes `install` find a page whose heading is
* `Installation`; without it a user typing a partial word gets zero
* results from a genuinely relevant page.
*/
function search(index, query, opts = {}) {
	const tokens = tokenize(query);
	if (tokens.length === 0) return [];
	const scored = [];
	for (const e of index.entries) {
		const titleTokens = new Set(tokenize(e.title ?? ""));
		const headingTokens = new Set(tokenize((e.headings ?? []).join(" ")));
		const excerptTokens = new Set(tokenize(e.excerpt ?? ""));
		let score = 0;
		for (const t of tokens) {
			if (matchesAny(t, titleTokens)) score += 3;
			if (matchesAny(t, headingTokens)) score += 2;
			if (matchesAny(t, excerptTokens)) score += 1;
		}
		if (score === 0) continue;
		scored.push({
			route: e.route,
			url: e.url,
			title: e.title,
			headings: e.headings ?? [],
			excerpt: e.excerpt,
			score
		});
	}
	scored.sort((a, b) => {
		if (b.score !== a.score) return b.score - a.score;
		return a.route < b.route ? -1 : a.route > b.route ? 1 : 0;
	});
	const limit = opts.limit ?? 20;
	return scored.slice(0, limit);
}
export { DeferredClient, MAX_BYTES, MAX_DEPTH, MAX_REFS, OdfError, attachDeferGlue, componentFor, createRouter, decodeIslandProps, decodeOdf, decodeOdfToJson, enhance, getRouteComponentSource, hydrateIsland, hydrateIslands, isParamSegment, loadMessages, loadScripts, pageComponentExport, parseStrategy, reportWebVitals, routeIdFor, scheduleScript, search, setMessageLocales, setRouteComponentSource, t, tokenize, wireDeferreds };

//# sourceMappingURL=client.js.map