# Application npm license evidence

`provenance.json` binds each fallback to an exact package/version/archive integrity. Normal package notices come directly from integrity-checked npm archives; these preserved texts cover packages whose archives omit a standalone license file. The generator never depends on installed `node_modules` or the ignored staged SDK.

Most fallbacks use the registry's exact published Git revision. `bplist-parser` and `imurmurhash` reproduce complete license sections from their published READMEs. For `boolbase`, the later maintainer license commit still declares 1.0.0 and its implementation is byte-identical to the published archive. For `structured-headers`, the first license commit still declares 0.4.1 and all six published TypeScript source files match; the file-by-file hashes are preserved separately. These are explicit source-equivalence findings, not claims that the original tarballs contained the later license files.

The published `@ubjs/core` and `@ubjs/react-native` `0.31.0-6` archives omit their
standalone license file. Their registry `gitHead` identifies upstream revision
`3dd1cd1353750dc3bfbbf4bb4d4ccde26b0fbd2e`; the fallback preserves that revision's
exact `LICENSE` text and its link to the MPL 2.0 terms. This supplies source
attribution; the existing MPL-2.0 policy finding remains unresolved.

The preserved fallback reviews for `casework` and `jimp-compact` remain unresolved: `casework` has a license declaration but no complete attributed text at the published revision; `jimp-compact` reproduces its explicitly attributed Jimp 0.16.1 terms, but its bundled dependencies and wrapper are not fully reviewed. Other missing archive notices and unapproved declarations are reported by exact package/version in the generated notice bundle. No copyright holder or legal text is inferred.

The collector records absent required peers separately in `unresolved_required_peers`; they contribute no selected archive bytes, but remain failures in the release audit and are displayed in the notice bundle. Missing required runtime dependencies still stop collection. An optional peer declaration on a parent package does not waive a child's required peer. Installed optional peers of registry packages remain in this conservative declaration inventory, including Router's testing-library/Jest path; this does not claim they are embedded in the application bundle.

The existing permissive policy is evaluated over SPDX AND/OR/WITH expressions. Unapproved identifiers, including file-level MPL dependencies without an application exception, fail the application audit. Producing a notice document does not waive missing attribution, approve license terms, or clear advisories. `npm-production-audit.py` defaults to the full production declaration closure; `--scope web` is only for the independent website/WASM qualification.

The application closure is conservative package-manifest reachability, including installed registry-package optional peers. Optional frameworks present only in source-library development locks do not become roots. This is not a claim that every CLI, compiler or test helper in the declaration closure is embedded in the final JavaScript bundle. Any narrower distribution claim needs artifact/entrypoint evidence.
