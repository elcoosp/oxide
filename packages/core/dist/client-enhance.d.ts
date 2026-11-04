/** Structural form surface (real element or test fake). */
export interface EnhanceForm {
  addEventListener(type: string, fn: (e: unknown) => void): void;
  removeEventListener(type: string, fn: (e: unknown) => void): void;
  getAttribute(name: string): string | null;
  hasAttribute(name: string): boolean;
  dispatchEvent?(ev: {
    type: string;
    detail?: unknown;
  }): boolean;
}
export interface EnhanceOptions {
  fetch?: typeof fetch;
  /** Extract the form body (default: `new FormData(form)`). */
  getFormData?: (form: EnhanceForm) => FormData;
  /**
   * Optimistic hook (§9.3): runs BEFORE the request with the form data, so
   * the adapter can apply an optimistic update (rollback via onError).
   */
  onSubmit?(formData: FormData, form: EnhanceForm): void | Promise<void>;
  /** Receive the parsed action result (form result data, §9.3). */
  applyResult?(result: unknown, form: EnhanceForm): void;
  /** Failure path — rollback optimistic state, surface the error. */
  onError?(error: unknown, form: EnhanceForm): void;
  /** Where results are dispatched when no applyResult is given. */
  baseLocation?: string;
}
export interface EnhanceSubmitEvent {
  preventDefault(): void;
}
/**
 * Wire progressive enhancement onto a form. Returns a disposer that removes
 * the submit listener. Actions are POST-only (§17.1); the body is FormData
 * for multipart forms and URLSearchParams otherwise.
 */
export declare function enhance(form: EnhanceForm, opts?: EnhanceOptions): () => void;
//# sourceMappingURL=client-enhance.d.ts.map