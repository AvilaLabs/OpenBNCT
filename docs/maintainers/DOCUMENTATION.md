# Maintain the OpenBNCT Handbook

The current user documentation lives in `docs/guide/`. `book.toml` builds it with **mdBook 0.5.4**, the Rust theme and optional Navy dark theme. Its public URL is <https://openbnct.avilalabs.org/docs/>.

Keep navigation organized by research tasks. Detailed usage references, ADRs and frozen benchmark/validation evidence remain outside the book. Link their exact configurations and qualification scope; never regenerate frozen evidence as part of a documentation change.

## Build and check

```bash
mdbook build
python3 scripts/check_handbook.py
npm ci --prefix docs/checks --ignore-scripts
docs/checks/node_modules/.bin/playwright install chromium
python3 -m http.server 8080 --directory dist --bind 127.0.0.1
# In a second terminal:
node scripts/docs_smoke.mjs
```

The checker verifies curated chapter membership, workspace version, local links/anchors and repository source paths. Browser checks cover navigation, search, Rust/Navy themes and a 390 px mobile layout. `HANDBOOK_URL` selects another local or live site.

On the owner's Linux workstation, every build/executable check must use the enforced cgroup and disk-backed `TMPDIR` required by root `AGENTS.md`. Do not run concurrent test or transport jobs. The CLI's existing documentation drift test also checks handbook command paths.

## Publish

Push reviewed changes to `main` after the required local gates. Watch the exact SHA's **ci**, **Build handbook** and **Deploy web workbench** runs. Do not report a queued or failing workflow as complete.

The existing Pages workflow builds the real browser workbench, licenses and notices, then builds/checks the handbook and places it at `crates/openbnct-gui/dist/docs/`. It tests the book from the complete bundle and uploads both the Pages artifact and a downloadable **openbnct-web** artifact. The deploy job publishes that bundle through the existing GitHub Pages environment and custom domain.

No new DNS configuration is needed. The workbench stays at `/` and the handbook lives at `/docs/`. Documentation-source changes trigger the Pages build; retired chapters disappear because the generated docs destination is replaced. The standalone **Build handbook** workflow supplies a separate `openbnct-handbook` artifact for documentation review.

After deployment, check the live workbench, Help → Handbook link, book navigation/search, benchmark chapter and mobile layout. To republish the current revision, dispatch the Pages workflow after checking the source SHA; avoid publishing unrelated local edits.

## Keep content current

Verify commands and defaults against the CLI, Python functions against the binding implementation, and units/geometry against Rust contracts. Distinguish current source from older packaged releases. Keep Python/PyPI availability current without rewriting historical ADRs.

Match physics in comparisons: source weighting, thermal scattering, material fractions, component responses, photon policy and normalization. Do not combine ratios from different configurations. Explain the graded region and acceptance criteria, retain Monte Carlo uncertainty, and keep normalized profile agreement separate from absolute dose. Research evidence does not establish clinical qualification.
