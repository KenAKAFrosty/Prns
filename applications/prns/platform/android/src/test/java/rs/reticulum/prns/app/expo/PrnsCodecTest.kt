package rs.reticulum.prns.app.expo

import org.junit.Assert.assertEquals
import org.junit.Assert.assertThrows
import org.junit.Test
import rs.reticulum.prns.app.bindings.*

class PrnsCodecTest {
  @Test fun generatedUtf8CodecReturnsOnlyWrittenBytes() {
    val text = "Rust stopped: café 🚀"
    val bytes = PrnsCodec.encode(text, FfiConverterString)
    assertEquals(4 + text.toByteArray(Charsets.UTF_8).size, bytes.size)
    assertEquals(text, PrnsCodec.decode(PrnsCodec.bytes(bytes), FfiConverterString))
  }
  @Test fun generatedStartCodecRejectsTrailingData() {
    val value = DevelopmentNodeStartInput(null)
    val bytes = PrnsCodec.encode(value, FfiConverterTypeDevelopmentNodeStartInput)
    assertEquals(listOf(0), bytes)
    assertEquals(value, PrnsCodec.decode(PrnsCodec.bytes(bytes), FfiConverterTypeDevelopmentNodeStartInput))
    assertThrows(IllegalArgumentException::class.java) {
      PrnsCodec.decode(PrnsCodec.bytes(bytes + 0), FfiConverterTypeDevelopmentNodeStartInput)
    }
  }
  @Test fun malformedExpoByteInputsCannotReachNative() {
    for (input in listOf(listOf(-1), listOf(256), List(65_537) { 0 })) {
      assertThrows(IllegalArgumentException::class.java) { PrnsCodec.bytes(input) }
    }
  }
  @Test fun generatedCodecPreservesExactUnsignedIntegers() {
    val bytes = PrnsCodec.encode(ULong.MAX_VALUE, FfiConverterULong)
    assertEquals(List(8) { 255 }, bytes)
    assertEquals(ULong.MAX_VALUE, PrnsCodec.decode(PrnsCodec.bytes(bytes), FfiConverterULong))
  }
}
