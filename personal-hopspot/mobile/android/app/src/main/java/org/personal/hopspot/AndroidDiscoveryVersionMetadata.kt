package org.personal.hopspot

internal enum class AndroidDiscoveryVersionMetadata {
    ImplicitV1,
    ExplicitV1;

    companion object {
        private const val EXPLICIT_VERSION_MINIMUM_API = 21

        fun forApiLevel(apiLevel: Int): AndroidDiscoveryVersionMetadata =
            if (apiLevel >= EXPLICIT_VERSION_MINIMUM_API) ExplicitV1 else ImplicitV1
    }
}
