# App native bindings

This package contains the ordinary generated APIs from the one `prns_chat_app` image.
The image selects the canonical host, identity, group, text, contact-card and
archive capabilities through `prns-service-runtime-uniffi`. The app has no matching domain
method inventory or copied records.

Import `@prns/app-bindings` on React Native to install the generated namespaces,
check their UniFFI ABI checksums and check app API version 3. The package exports
the owning namespace names. Direct subpaths such as
`@prns/app-bindings/prns_groups` expose the full generated domain module;
initialize the native entry once before using those modules. Plain `index.ts`
exports values/types without installing native bindings.

The canonical host converter is a re-export from `personal-rns-expo/bindings/values`.
That SDK is staged in `applications/target/react-native-sdk` from the same image
recipe. It alone packages the selected native framework/library; this package
adds no second native image. Generated Swift/Kotlin are retained as one complete
module set for review and direct foreign consumers. The staged SDK compiles all
native namespaces from this same image together, so cross-namespace Swift types
share its `PrnsHostExpo` module and Kotlin keeps its owning packages. Product
platform code imports those SDK declarations; it must not compile the review
outputs a second time.

Run `npm --prefix applications run api:generate` to regenerate all languages and
stage the SDK. `api:check` verifies complete generated ownership, including stale
files. `sdk:stage` verifies tracked bindings before materializing the disposable
SDK package. The app maintains image selection and its product API version,
not a parallel domain-method inventory or converter implementation.
