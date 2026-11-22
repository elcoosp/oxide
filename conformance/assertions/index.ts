// V2-R01: the assertion registry. Collects the migrated v0.1 set (`v01.ts`)
// and every C25–C47 per-id file into one array the runner dispatches. The
// skip engine (`skip.ts`) decides per (assertion, target) cell whether an
// assertion runs, skips (capability-unsupported or ci-bound), or blocks on
// failure (a Full capability that fails).

import type { Assertion } from './types.ts'
import { V01_ASSERTIONS } from './v01.ts'
import c25 from './c25.ts'
import c26 from './c26.ts'
import c27 from './c27.ts'
import c28 from './c28.ts'
import c29 from './c29.ts'
import c30 from './c30.ts'
import c31 from './c31.ts'
import c32 from './c32.ts'
import c33 from './c33.ts'
import c34 from './c34.ts'
import c35 from './c35.ts'
import c36 from './c36.ts'
import c37 from './c37.ts'
import c38 from './c38.ts'
import c39 from './c39.ts'
import c40 from './c40.ts'
import c41 from './c41.ts'
import c42 from './c42.ts'
import c43 from './c43.ts'
import c44 from './c44.ts'
import c45 from './c45.ts'
import c46 from './c46.ts'
import c47 from './c47.ts'

/** Every registered assertion, in stable order: the v0.1 set first (the §22
 *  contract), then C25–C47 (the §V2-R01 feature set, in spec-table order). */
export const ALL_ASSERTIONS: readonly Assertion[] = [
  ...V01_ASSERTIONS,
  c25, c26, c27, c28, c29, c30, c31, c32, c33, c34, c35, c36, c37, c38, c39, c40, c41, c42, c43, c44, c45, c46, c47,
]

/** Look up an assertion by its stable id. */
export function byId(id: string): Assertion | undefined {
  return ALL_ASSERTIONS.find((a) => a.id === id)
}

/** The set of ids the registry carries (the table's row vocabulary). */
export const ASSERTION_IDS: readonly string[] = ALL_ASSERTIONS.map((a) => a.id)
