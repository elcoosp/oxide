// Node ESM hook registration shim (Node 22+).
//
// `--experimental-loader <path>` still works but is deprecated; Node 22
// warns and points at the newer `register()` API. This module is invoked
// via `--import <shim>` instead, so the worker startup is warning-free.
//
// The shim is intentionally tiny: it registers the sibling `tsx-loader.mjs`
// with the shim's own URL as the parent, so relative specifiers inside the
// loader resolve from the worker directory.
import { register } from 'node:module'
register('./tsx-loader.mjs', import.meta.url)
