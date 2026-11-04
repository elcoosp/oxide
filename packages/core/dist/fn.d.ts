/**
 * Call the server function named by `id` with `args`. Returns the resolved
 * value, or throws an `Error` whose message is the server's diagnostic.
 *
 * The `content-type` is `application/oxide-fn` — the compiler's stub used
 * this marker and the endpoint matches on it, so a project can tell a
 * server-fn POST from a form action POST in a log.
 */
export declare function __oxide_call_fn(id: string, args: unknown[]): Promise<unknown>;
//# sourceMappingURL=fn.d.ts.map