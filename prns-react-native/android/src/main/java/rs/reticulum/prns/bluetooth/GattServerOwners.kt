// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 The Prns Authors

package rs.reticulum.prns.bluetooth

import java.util.LinkedHashMap

/**
 * Android server callbacks identify only an address, not a connection or notification.
 * Keep submitted owners after retirement. A timed-out address stays quarantined until
 * server teardown; guessing a replacement owner would let a late disconnect close it.
 * All retained addresses count toward capacity, including tombstones.
 * The transport lifecycle lock serializes all calls, including epoch checks and OS submission.
 */
internal class GattServerOwners<T : Any>(private val capacity: Int) {
    private class Slot<T>(var owner: T?) {
        var notification: T? = null
        var disconnectPending = false
        var quarantined = false
    }

    private val slots = LinkedHashMap<String, Slot<T>>()
    var epoch: Long = 0
        private set

    fun canRegister(address: String): Boolean = !slots.containsKey(address) && slots.size < capacity

    fun register(address: String, owner: T): Boolean {
        if (slots.containsKey(address) || slots.size >= capacity) return false
        slots[address] = Slot(owner)
        return true
    }

    fun beginNotification(address: String, owner: T): Boolean {
        val slot = slots[address] ?: return false
        if (slot.owner !== owner || slot.notification != null || slot.quarantined) return false
        slot.notification = owner
        return true
    }

    fun cancelNotification(address: String, owner: T) {
        val slot = slots[address] ?: return
        if (slot.notification !== owner) return
        slot.notification = null
        prune(address, slot)
    }

    fun completeNotification(callbackEpoch: Long, address: String): T? {
        if (callbackEpoch != epoch) return null
        val slot = slots[address] ?: return null
        val owner = slot.notification
        slot.notification = null
        prune(address, slot)
        return owner
    }

    fun retire(address: String, owner: T, disconnected: Boolean, uncertain: Boolean) {
        val slot = slots[address] ?: return
        if (slot.owner !== owner) return
        slot.owner = null
        slot.disconnectPending = !disconnected
        slot.quarantined = uncertain
        prune(address, slot)
    }

    fun disconnected(callbackEpoch: Long, address: String): T? {
        if (callbackEpoch != epoch) return null
        val slot = slots[address] ?: return null
        if (slot.disconnectPending || slot.quarantined) {
            slot.disconnectPending = false
            prune(address, slot)
            return null
        }
        return slot.owner
    }

    fun reset() {
        epoch++
        slots.clear()
    }

    private fun prune(address: String, slot: Slot<T>) {
        if (slot.owner == null && slot.notification == null &&
            !slot.disconnectPending && !slot.quarantined
        ) slots.remove(address)
    }
}
