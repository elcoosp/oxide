interface HotContext {
  readonly url: string;
  data: Record<string, unknown>;
  accept(cb?: (mod: unknown) => void): void;
  dispose(cb?: () => void): void;
  invalidate(): void;
}
export { HotContext as t };
//# sourceMappingURL=hot-DxEu1ml0.d.ts.map