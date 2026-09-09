package rs.reticulum.prns.app.expo

import java.nio.ByteBuffer
import java.nio.ByteOrder
import rs.reticulum.prns.app.bindings.FfiConverter

/** Owned Expo byte arrays; all domain layout belongs to the generated codecs. */
internal object PrnsCodec {
  fun bytes(input: List<Int>): ByteArray {
    require(input.size <= 65_536 && input.all { it in 0..255 }) { "Native input must contain at most 65536 bytes" }
    return ByteArray(input.size) { input[it].toByte() }
  }

  fun <T> decode(bytes: ByteArray, converter: FfiConverter<T, *>): T {
    require(bytes.size <= 65_536) { "Native input is too large" }
    val buffer = ByteBuffer.wrap(bytes).order(ByteOrder.BIG_ENDIAN)
    val value = converter.read(buffer)
    require(!buffer.hasRemaining()) { "Native input contains trailing bytes" }
    return value
  }

  fun <T> encode(value: T, converter: FfiConverter<T, *>): List<Int> {
    val size = converter.allocationSize(value)
    require(size <= Int.MAX_VALUE.toULong()) { "Native result is too large" }
    val buffer = ByteBuffer.allocate(size.toInt()).order(ByteOrder.BIG_ENDIAN)
    converter.write(value, buffer)
    // allocationSize is an upper bound (notably for UTF-8 strings).
    return buffer.array().take(buffer.position()).map { it.toInt() and 0xff }
  }
}
