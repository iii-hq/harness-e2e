# Harness E2E design system

Phase one provides isolated foundations and primitives. Production dashboard
pages do not import these files yet.

## Internal demonstration

Run the dashboard frontend and open `/design-system.html`:

```bash
pnpm --dir dashboard dev
```

The production build includes the same internal page as a separate Vite
entrypoint without changing the dashboard hash router.

## Future adoption

Import the shared styles once at the application entrypoint, then consume the
typed primitives:

```tsx
import '@/design-system/styles.css'
import { Button, StatusBadge } from '@/design-system'
```

Do not replace operational meanings while migrating. `passed`, `failed`,
`inconclusive`, `unavailable`, `hard_gate`, and `recommendation` are distinct
states. Missing values remain `Not reported`; advisory recommendations do not
override deterministic gates.

GSAP is not part of any primitive. It is used only by the internal narrative
demo and is reserved for Overview, onboarding, and narrative comparison work.
