// src/client/messages.ts
const cache = /* @__PURE__ */ new Map();
let activeLocale = null;
let inflight = null;
/** The locale the page was rendered in — `<html lang>` is authoritative
*  (the server sets it from the resolved locale), falling back to the
*  document's `data-locale` if present. */
function detectLocale() {
	if (activeLocale) return activeLocale;
	const doc = globalThis.document;
	if (doc?.documentElement?.lang) {
		activeLocale = doc.documentElement.lang;
		return activeLocale;
	}
	const dl = doc?.documentElement?.getAttribute?.("data-locale");
	if (dl) {
		activeLocale = dl;
		return activeLocale;
	}
	activeLocale = "en";
	return activeLocale;
}
/** Derive the route key the build uses for a per-route subset. The build
*  keys on the route directory with any trailing `/page` stripped and slashes
*  replaced by underscores (`routes/blog/[slug]` → `blog_[slug]`); this
*  reproduces it from the ODF payload's `route` field, or from
*  `location.pathname` when the payload is absent. */
function routeKeyFromPage() {
	const route = globalThis.__oxide?.route;
	if (typeof route === "string" && route.length > 0) {
		const stripped = route.endsWith("/page") ? route.slice(0, -5) : route;
		return (stripped.startsWith("routes/") ? stripped.slice(7) : stripped).replace(/\//g, "_");
	}
	return null;
}
/** Load (and cache) the active locale's catalog. Tries the per-route subset
*  first (`/client/messages/<locale>/<route>.json`), falling back to the full
*  catalog (`/client/messages/<locale>.json`). Resolves to an empty map on
*  any failure — a missing catalog must not break a page; `t()` then returns
*  the key, which is a visible-and-recoverable degradation, not a crash. */
/**
* §16.4: whether this build emitted any message catalogs.
*
* Set from `client.manifest.json`'s `i18n_locales` (the CLI stamps it at
* build time). `undefined` means "not told" — the fetch proceeds, which is
* the pre-existing behaviour for a caller that never registered a manifest.
* An empty array means "the project has no i18n", and the fetch is skipped:
* without this, every page load of a non-i18n app requests
* `/client/messages/<locale>.json`, gets a 404, and Chromium reports it as a
* console error.
*/
let hasCatalogs;
/** Register the build's locale list (from the client manifest). */
function setMessageLocales(locales) {
	hasCatalogs = locales === void 0 ? void 0 : locales.length > 0;
}
async function loadMessages(locale) {
	const l = locale ?? detectLocale();
	const cached = cache.get(l);
	if (cached) return cached;
	if (inflight) return inflight;
	if (hasCatalogs === false) return {};
	inflight = (async () => {
		try {
			const merged = {};
			let subset = null;
			const routeKey = routeKeyFromPage();
			if (routeKey) try {
				const sub = await globalThis.fetch(`/client/messages/${encodeURIComponent(l)}/${encodeURIComponent(routeKey)}.json`);
				if (sub.ok) subset = (await sub.json()).messages ?? {};
			} catch {}
			try {
				const res = await globalThis.fetch(`/client/messages/${encodeURIComponent(l)}.json`);
				if (res.ok) {
					const body = await res.json();
					Object.assign(merged, body.messages ?? {});
				}
			} catch {}
			if (subset) Object.assign(merged, subset);
			const msgs = Object.freeze(merged);
			cache.set(l, msgs);
			return msgs;
		} catch {
			return {};
		} finally {
			inflight = null;
		}
	})();
	return inflight;
}
/** Synchronously look up a message. Returns `key` when the catalog has not
*  been loaded yet or the key is absent — a caller that needs the value
*  before load can `await loadMessages()` first. */
function t(key, values) {
	const l = detectLocale();
	const msgs = cache.get(l) ?? {};
	let out = msgs[key];
	if (out === void 0) {
		const suffix = "." + key;
		for (const k of Object.keys(msgs)) if (k.endsWith(suffix)) {
			out = msgs[k];
			break;
		}
	}
	out = out ?? key;
	if (values) out = formatIcu(out, values, l);
	return out;
}
/** Format an ICU MessageFormat string with the browser's own `Intl` APIs.
*  Unknown placeholders are left intact (matching the previous substitution
*  behaviour) so a missing value is visible rather than silently empty. */
function formatIcu(message, values, locale = detectLocale(), pound) {
	let out = "";
	let i = 0;
	while (i < message.length) {
		const c = message[i];
		if (c === "'") {
			const quoted = readQuoted(message, i);
			if (quoted) {
				out += quoted.value;
				i = quoted.next;
			} else {
				out += c;
				i += 1;
			}
			continue;
		}
		if (c === "#") {
			out += pound === void 0 ? "#" : formatNumber(pound, locale);
			i += 1;
			continue;
		}
		if (c !== "{") {
			out += c;
			i += 1;
			continue;
		}
		const close = findClosingBrace(message, i);
		if (close < 0) {
			out += message.slice(i);
			break;
		}
		out += formatArgument(message.slice(i + 1, close), values, locale, pound);
		i = close + 1;
	}
	return out;
}
function readQuoted(s, start) {
	const next = s[start + 1];
	if (next === "'") return {
		value: "'",
		next: start + 2
	};
	if (next === void 0 || !"{#}|".includes(next)) return null;
	let value = "";
	let i = start + 1;
	while (i < s.length) {
		if (s[i] === "'") {
			if (s[i + 1] === "'") {
				value += "'";
				i += 2;
				continue;
			}
			return {
				value,
				next: i + 1
			};
		}
		value += s[i];
		i += 1;
	}
	return {
		value,
		next: i
	};
}
function findClosingBrace(s, open) {
	let depth = 0;
	let i = open;
	while (i < s.length) {
		const c = s[i];
		if (c === "'") {
			const quoted = readQuoted(s, i);
			if (quoted) {
				i = quoted.next;
				continue;
			}
		}
		if (c === "{") depth += 1;
		else if (c === "}") {
			depth -= 1;
			if (depth === 0) return i;
		}
		i += 1;
	}
	return -1;
}
function formatArgument(inner, values, locale, pound) {
	const comma = inner.indexOf(",");
	if (comma < 0) {
		const name = inner.trim();
		if (!(name in values)) return `{${name}}`;
		return String(values[name]);
	}
	const name = inner.slice(0, comma).trim();
	const rest = inner.slice(comma + 1).trim();
	if (!(name in values)) return `{${inner}}`;
	const value = values[name];
	if (value === void 0) return `{${inner}}`;
	const second = rest.indexOf(",");
	const type = (second < 0 ? rest : rest.slice(0, second)).trim();
	const style = second < 0 ? "" : rest.slice(second + 1).trim();
	switch (type) {
		case "plural":
		case "selectordinal": return formatPlural(value, style, locale, type === "selectordinal", values, pound);
		case "select": return formatSelect(value, style, locale, values, pound);
		case "number": return formatNumberValue(value, style, locale);
		case "date":
		case "time": return formatDateTime(value, style, locale, type);
		default: return String(value);
	}
}
function parseOptions(source) {
	const options = /* @__PURE__ */ new Map();
	let offset = 0;
	let i = 0;
	while (i < source.length) {
		while (i < source.length && /\s/.test(source.charAt(i))) i += 1;
		if (i >= source.length) break;
		const keyStart = i;
		while (i < source.length && !/\s/.test(source[i]) && source[i] !== "{") i += 1;
		const key = source.slice(keyStart, i);
		while (i < source.length && /\s/.test(source.charAt(i))) i += 1;
		if (key.startsWith("offset:")) {
			const n = Number(key.slice(7));
			if (Number.isFinite(n)) offset = n;
			continue;
		}
		if (source[i] !== "{") {
			i += 1;
			continue;
		}
		const close = findClosingBrace(source, i);
		if (close < 0) break;
		options.set(key, source.slice(i + 1, close));
		i = close + 1;
	}
	return {
		options,
		offset
	};
}
function formatPlural(value, style, locale, ordinal, values, outerPound) {
	const n = Number(value);
	if (!Number.isFinite(n)) return String(value);
	const { options, offset } = parseOptions(style);
	const exact = options.get("=" + String(n - offset));
	if (exact !== void 0) return formatIcu(exact, values, locale, n - offset);
	let category = "other";
	try {
		category = new Intl.PluralRules(locale, { type: ordinal ? "ordinal" : "cardinal" }).select(n - offset);
	} catch {
		category = n - offset === 1 ? "one" : "other";
	}
	const branch = options.get(category) ?? options.get("other");
	return branch === void 0 ? "" : formatIcu(branch, values, locale, n - offset);
}
function formatSelect(value, style, locale, values, pound) {
	const { options } = parseOptions(style);
	const branch = options.get(String(value)) ?? options.get("other");
	return branch === void 0 ? "" : formatIcu(branch, values, locale, pound);
}
function formatNumber(n, locale) {
	try {
		return new Intl.NumberFormat(locale).format(n);
	} catch {
		return String(n);
	}
}
function formatNumberValue(value, style, locale) {
	const n = Number(value);
	if (!Number.isFinite(n)) return String(value);
	const s = style.trim();
	try {
		if (s === "integer") return new Intl.NumberFormat(locale, { maximumFractionDigits: 0 }).format(n);
		if (s === "percent") return new Intl.NumberFormat(locale, { style: "percent" }).format(n);
		if (s.startsWith("currency:")) return new Intl.NumberFormat(locale, {
			style: "currency",
			currency: s.slice(9)
		}).format(n);
	} catch {}
	return formatNumber(n, locale);
}
function formatDateTime(value, style, locale, kind) {
	const d = new Date(value);
	if (Number.isNaN(d.getTime())) return String(value);
	const s = style.trim();
	const dateStyle = [
		"short",
		"medium",
		"long",
		"full"
	].includes(s) ? s : void 0;
	try {
		return new Intl.DateTimeFormat(locale, {
			...kind === "date" ? { dateStyle: dateStyle ?? "medium" } : {},
			...kind === "time" ? { timeStyle: dateStyle ?? "medium" } : {}
		}).format(d);
	} catch {
		return String(value);
	}
}
/** Test-only: reset the module-level cache. */
function _resetMessagesForTest() {
	cache.clear();
	activeLocale = null;
	inflight = null;
}
export { _resetMessagesForTest, formatIcu, loadMessages, setMessageLocales, t };

//# sourceMappingURL=client-messages.js.map