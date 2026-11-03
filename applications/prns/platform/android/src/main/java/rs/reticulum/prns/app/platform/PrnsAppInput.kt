package rs.reticulum.prns.app.platform

/** Only nonsecret product selection is saved or put in a service intent. */
internal data class PrnsAppInput(val create: Boolean, val bluetooth: Boolean) {
  fun encode(): ByteArray = byteArrayOf(1, if (create) 1 else 0, if (bluetooth) 1 else 0)
  fun forRestoration(): PrnsAppInput = copy(create = false)
  companion object {
    fun decode(value: ByteArray): PrnsAppInput {
      require(value.size == 3 && value[0] == 1.toByte() &&
        value[1] in 0..1 && value[2] in 0..1) { "Invalid PRNS startup input" }
      return PrnsAppInput(value[1] == 1.toByte(), value[2] == 1.toByte())
    }
    fun from(mode: String, bluetooth: Boolean): PrnsAppInput {
      require(mode == "create" || mode == "open") { "Choose create or open for this installation" }
      return PrnsAppInput(mode == "create", bluetooth)
    }
  }
}
