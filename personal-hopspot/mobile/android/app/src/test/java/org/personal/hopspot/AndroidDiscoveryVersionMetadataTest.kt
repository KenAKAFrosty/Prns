package org.personal.hopspot

import org.junit.Assert.assertEquals
import org.junit.Test

class AndroidDiscoveryVersionMetadataTest {
    @Test
    fun api19And20UseImplicitV1WhileApi21AndLaterPublishExplicitV1() {
        assertEquals(
            listOf(
                AndroidDiscoveryVersionMetadata.ImplicitV1,
                AndroidDiscoveryVersionMetadata.ImplicitV1,
                AndroidDiscoveryVersionMetadata.ExplicitV1,
                AndroidDiscoveryVersionMetadata.ExplicitV1,
            ),
            listOf(19, 20, 21, 36).map(AndroidDiscoveryVersionMetadata::forApiLevel),
        )
    }
}
