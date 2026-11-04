// src/client/vitals.ts
function g() {
	if (typeof globalThis.window === "undefined") return void 0;
	return globalThis;
}
/**
* Install the reporting hook. Returns a disposer that disconnects the
* observers and clears any pending timer. Idempotent per call — calling twice
* installs two independent hooks (the caller owns the disposer).
*
* No-op (returns a no-op disposer) outside the browser.
*/
function reportWebVitals(opts = {}) {
	const w = g();
	if (!w || !w.PerformanceObserver) return () => {};
	const endpoint = opts.endpoint ?? "/_oxide/vitals";
	const route = opts.route ?? w.location?.pathname ?? "/";
	const want = new Set(opts.metrics ?? [
		"LCP",
		"CLS",
		"INP",
		"FCP",
		"TTFB"
	]);
	const report = opts.report !== false;
	const queue = [];
	let timer;
	let disposed = false;
	const observers = [];
	const flush = () => {
		if (disposed) return;
		if (timer !== void 0) {
			clearTimeout(timer);
			timer = void 0;
		}
		if (!report || queue.length === 0) {
			queue.length = 0;
			return;
		}
		const batch = queue.splice(0, queue.length);
		try {
			const body = JSON.stringify(batch);
			if (w.navigator?.sendBeacon) w.navigator.sendBeacon(endpoint, new Blob([body], { type: "application/json" }));
			else fetch(endpoint, {
				method: "POST",
				headers: { "content-type": "application/json" },
				body,
				keepalive: true
			}).catch(() => {});
		} catch {}
	};
	const push = (name, value) => {
		if (disposed || !want.has(name)) return;
		const sample = {
			name,
			value,
			route
		};
		try {
			opts.onSample?.(sample);
		} catch {}
		queue.push(sample);
		if (timer === void 0) timer = setTimeout(flush, 1e3);
	};
	const observe = (type, handle) => {
		try {
			const obs = new w.PerformanceObserver((list) => {
				for (const e of list.getEntries()) handle(e);
			});
			obs.observe({
				type,
				buffered: true,
				durationThreshold: 40
			});
			observers.push(obs);
		} catch {}
	};
	let lcp = 0;
	observe("largest-contentful-paint", (e) => {
		if (typeof e.startTime === "number") lcp = e.startTime;
	});
	let cls = 0;
	observe("layout-shift", (e) => {
		if (!e.hadRecentInput && typeof e.value === "number") cls += e.value;
	});
	let inp = 0;
	observe("event", (e) => {
		if (typeof e.duration === "number" && e.duration > inp) inp = e.duration;
	});
	const paintEntries = w.performance?.getEntriesByType("paint") ?? [];
	for (const e of paintEntries) if (e.name === "first-contentful-paint" && typeof e.startTime === "number") push("FCP", e.startTime);
	const nav = w.performance?.getEntriesByType("navigation") ?? [];
	if (nav[0] && typeof nav[0].responseStart === "number") push("TTFB", nav[0].responseStart);
	const flushFinal = () => {
		if (lcp > 0) push("LCP", lcp);
		if (cls > 0) push("CLS", cls);
		if (inp > 0) push("INP", inp);
		flush();
	};
	const onPageHide = () => flushFinal();
	const onVisibility = () => {
		if (w.document?.visibilityState === "hidden") flushFinal();
	};
	w.addEventListener("pagehide", onPageHide);
	w.document?.addEventListener("visibilitychange", onVisibility);
	return () => {
		disposed = true;
		for (const o of observers) o.disconnect();
		if (timer !== void 0) clearTimeout(timer);
	};
}
export { reportWebVitals as t };

//# sourceMappingURL=vitals-L5C-0ePE.js.map