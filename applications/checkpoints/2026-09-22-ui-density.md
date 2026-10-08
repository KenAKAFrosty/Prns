# Compact app layout and clearer grouping

This is a presentation-only follow-up to the
[remote settings workflows](2026-09-21-remote-settings-workflows.md). It changes
neither the native contract nor firmware. No branch is pushed in this pass.

## Changes

- `8d7067203`: tighter shared page/card spacing and headings; compact label/value
  rows with stacked long-text, narrow-screen and enlarged-text fallbacks;
  reusable card headers, divided sections and wrapping control rows. Buttons and
  fields retain a 48-logical-pixel minimum touch height, with text scaling enabled.
- `b63b1e57b`: paired nodes first on Nodes, local recovery/status controls grouped
  below; compact remote-settings navigation and refresh header; clear settings,
  connection-control and diagnostic sections. Paired actions and numeric fields
  share rows where they fit. Reading and editing no longer repeat the same LoRa
  values simultaneously. Confirmation, failure and uncertain-outcome warnings
  remain visible where relevant.
- `efc001c38`: Add contact stays above the list, contact summaries omit duplicated
  identity details, message sharing moves under Messaging options, and message
  metadata moves under Message details. Unverified-message warnings and retry/
  cancel actions remain visible. Help reflects implemented remote settings.
- `cabb9fad1`: physical-review follow-up removes repeated permission/address
  summaries from list pages, shortens a wrapping navigation label while retaining
  its accessible name, collapses the long region selector until requested, and
  lets very large navigation labels use two rows rather than splitting words.

No runtime lifecycle, permission, storage, write-admission or pairing behavior is
intentionally changed. Existing lifecycle and mutation tests remain part of the
full app suite.

## Host validation

- 319 tests in 38 app suites pass, including new grouping, ordering, disclosure,
  narrow-layout, font-scale and minimum-touch-height checks.
- App formatting, lint and source/test/tools TypeScript checks pass.
- Route/configuration checks and web export pass. Web export is a build check,
  not a claim that the currently native-only features run in the browser.

Logs and before/after device screenshots live under
`scratch/prns-app/2026-09-22/ui-density/` (`*-final.log` records the follow-up gates).

## Galaxy S9+ review

The first standalone APK at `efc001c38` was installed over retained data at
08:09:45 EDT, SHA256
`b5c7ed263da2d561044a05e51a1d2afdac1f2022570fdaca988c84fb483681b8`.
The existing paired E290 reconnected and returned a fresh LoRa settings read.
The initial interface card fits above the fold; before this pass its first value
was below the fold. All six LoRa profile rows are now visible from the top after
loading. Group dividers, headers and paired controls were visually inspected.

Typing the existing 22 dBm value in the compact radio editor kept the complete
field and cursor above the keyboard. The draft was abandoned, not saved; no
board-setting mutation, firmware flash or pairing change was needed.

Contacts empty state, the retained populated Inbox, message summaries and the
Message details disclosure were exercised. Populated Contacts rendering is
covered by host tests, not a newly created physical test contact. A 1.5x text
trial confirmed settings tabs and values reflow, and exposed a bottom-navigation
label split addressed in `cabb9fad1`. The phone's original 1.1 font scale was
restored; display density was not changed.

## Final installed build

The standalone APK at `cabb9fad1` was installed over retained data at 08:18:30 EDT.
SHA256: `3614ee7fee0419df0ceef7923eaa576619d3cd4b25f4fca96b4739b389dec49a`.
The canonical build passed its native Android unit tests, package/native-library
checks and bundled-JavaScript check; it retains Android 10 minimum and local
development signing. No Metro server is needed.

Cold launch retained the paired node and showed the final compact Nodes summary.
A fresh interface read populated the radio editor. The collapsed region selector
kept all six numeric fields visible together, expanded on Change region, and
collapsed after reselecting the existing US 915 draft. No Save was submitted.

The final 1.5x navigation check kept Contacts on one line; at 2x the five links
reflowed into two readable rows. Both were visually inspected. The original
1.1 font scale was restored afterward. Final screenshots are `nodes-final.png`,
`editor-final.png`, `navigation-font-1.5-final.png` and `navigation-font-2-final.png`
in the evidence directory. No iOS installation or browser interaction is claimed.
