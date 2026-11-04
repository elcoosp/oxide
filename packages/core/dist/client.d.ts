import { a as getRouteComponentSource, c as pageComponentExport, i as componentFor, l as routeIdFor, n as RouteComponentSource, o as isParamSegment, r as RouteManifestEntry, t as ClientManifestLike, u as setRouteComponentSource } from "./route-manifest-CpS1nShA.js";
import { t as HotContext } from "./hot-DxEu1ml0.js";
import { DeferGlue, DeferGlueOptions, DeferredClient, SwapDetail, WireOptions, attachDeferGlue, wireDeferreds } from "./client-defer-glue.js";
import { EnhanceForm, EnhanceOptions, EnhanceSubmitEvent, enhance } from "./client-enhance.js";
import { Router, RouterDoc, RouterLink, RouterOptions, createRouter } from "./client-router.js";
import { IslandCtx, IslandHost, IslandsAdapter, IslandsOptions, ParsedStrategy, decodeIslandProps, hydrateIsland, hydrateIslands, parseStrategy } from "./client-islands.js";
import { a as VitalSample, c as MAX_BYTES, d as OdfError, f as decodeOdf, i as scheduleScript, l as MAX_DEPTH, n as ScriptStrategy, o as VitalsOptions, p as decodeOdfToJson, r as loadScripts, s as reportWebVitals, t as ScriptOptions, u as MAX_REFS } from "./scripts--om8cvP3.js";
import { Messages, loadMessages, setMessageLocales, t } from "./client-messages.js";
export interface SearchEntry {
  route: string;
  url: string;
  title?: string;
  headings?: string[];
  excerpt?: string;
}
export interface SearchIndex {
  version: number;
  entries: SearchEntry[];
}
export interface SearchResult {
  route: string;
  url: string;
  title: string | undefined;
  headings: string[];
  excerpt: string | undefined;
  score: number;
}
/**
 * Tokenize by lowercasing and splitting on any non-alphanumeric run.
 * Drops single-character tokens (`a`, `of`) — a 1-char query rarely
 * discriminates and inflates the score for common words.
 */
export declare function tokenize(s: string): string[];
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
export declare function search(index: SearchIndex, query: string, opts?: {
  limit?: number;
}): SearchResult[];
export { type ClientManifestLike, type DeferGlue, type DeferGlueOptions, DeferredClient, type EnhanceForm, type EnhanceOptions, type EnhanceSubmitEvent, type HotContext, type IslandCtx, type IslandHost, type IslandsAdapter, type IslandsOptions, MAX_BYTES, MAX_DEPTH, MAX_REFS, type Messages, OdfError, type ParsedStrategy, type RouteComponentSource, type RouteManifestEntry, type Router, type RouterDoc, type RouterLink, type RouterOptions, type ScriptOptions, type ScriptStrategy, type SwapDetail, type VitalSample, type VitalsOptions, type WireOptions, attachDeferGlue, componentFor, createRouter, decodeIslandProps, decodeOdf, decodeOdfToJson, enhance, getRouteComponentSource, hydrateIsland, hydrateIslands, isParamSegment, loadMessages, loadScripts, pageComponentExport, parseStrategy, reportWebVitals, routeIdFor, scheduleScript, setMessageLocales, setRouteComponentSource, t, wireDeferreds };
//# sourceMappingURL=client.d.ts.map