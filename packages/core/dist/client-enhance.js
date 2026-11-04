import { a as decodeOdf } from "./odf-decode-D1PLHGuy.js";
// src/client/enhance.ts
/**
* Wire progressive enhancement onto a form. Returns a disposer that removes
* the submit listener. Actions are POST-only (§17.1); the body is FormData
* for multipart forms and URLSearchParams otherwise.
*/
function enhance(form, opts = {}) {
	const doFetch = opts.fetch ?? ((...a) => globalThis.fetch(...a));
	const getFormData = opts.getFormData ?? ((f) => new FormData(f));
	const onSubmit = async (e) => {
		e.preventDefault();
		const formData = getFormData(form);
		const target = form.getAttribute("action") ?? opts.baseLocation ?? "/";
		const body = (form.getAttribute("enctype") ?? "").includes("multipart/form-data") || [...formData.values()].some((v) => typeof v === "object" && v !== null) ? formData : new URLSearchParams([...formData.entries()].map(([k, v]) => [k, String(v)]));
		try {
			await opts.onSubmit?.(formData, form);
			const res = await doFetch(target, {
				method: "POST",
				body,
				headers: { accept: "application/oxide+odf, application/json, text/plain" }
			});
			if (!res.ok) throw new Error(`action failed: ${res.status}`);
			const result = await parseResult(res);
			if (opts.applyResult) opts.applyResult(result, form);
			else form.dispatchEvent?.({
				type: "ox:form-result",
				detail: result
			});
		} catch (err) {
			if (opts.onError) opts.onError(err, form);
			else form.dispatchEvent?.({
				type: "ox:form-error",
				detail: err
			});
		}
	};
	form.addEventListener("submit", onSubmit);
	return () => form.removeEventListener("submit", onSubmit);
}
async function parseResult(res) {
	const type = ((res.headers.get("content-type") ?? "").split(";").at(0) ?? "").trim();
	if (type === "application/oxide+odf" || type === "application/octet-stream") return decodeOdf(new Uint8Array(await res.arrayBuffer()));
	if (type === "application/json") return res.json();
	return res.text();
}
export { enhance };

//# sourceMappingURL=client-enhance.js.map