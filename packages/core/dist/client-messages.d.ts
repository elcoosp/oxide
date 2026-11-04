/** A loaded locale catalog, keyed by message id. */
export type Messages = Readonly<Record<string, string>>;
/** Register the build's locale list (from the client manifest). */
export declare function setMessageLocales(locales: string[] | undefined): void;
export declare function loadMessages(locale?: string): Promise<Messages>;
/** Synchronously look up a message. Returns `key` when the catalog has not
 *  been loaded yet or the key is absent — a caller that needs the value
 *  before load can `await loadMessages()` first. */
export declare function t(key: string, values?: Record<string, string | number>): string;
/** Format an ICU MessageFormat string with the browser's own `Intl` APIs.
 *  Unknown placeholders are left intact (matching the previous substitution
 *  behaviour) so a missing value is visible rather than silently empty. */
export declare function formatIcu(message: string, values: Readonly<Record<string, string | number>>, locale?: string, pound?: number): string;
/** Test-only: reset the module-level cache. */
export declare function _resetMessagesForTest(): void;
//# sourceMappingURL=client-messages.d.ts.map