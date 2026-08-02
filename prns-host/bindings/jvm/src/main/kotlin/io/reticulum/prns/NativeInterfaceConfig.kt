package io.reticulum.prns

import com.sun.jna.Pointer
import com.sun.jna.Structure

@Structure.FieldOrder("structSize", "baud", "dataBits", "parity", "stopBits")
internal open class NativeSerialLineConfig : Structure() {
    @JvmField
    var structSize: SizeT = SizeT()

    @JvmField
    var baud: Int = 0

    @JvmField
    var dataBits: Int = 0

    @JvmField
    var parity: Int = 0

    @JvmField
    var stopBits: Int = 0

    class ByValue : NativeSerialLineConfig(), Structure.ByValue
}

@Structure.FieldOrder(
    "structSize",
    "frequencyHz",
    "bandwidthHz",
    "txPowerDbm",
    "spreadingFactor",
    "codingRate",
)
internal open class NativeRNodeRadioConfig : Structure() {
    @JvmField
    var structSize: SizeT = SizeT()

    @JvmField
    var frequencyHz: Long = 0

    @JvmField
    var bandwidthHz: Int = 0

    @JvmField
    var txPowerDbm: Short = 0

    @JvmField
    var spreadingFactor: Byte = 0

    @JvmField
    var codingRate: Byte = 0

    class ByValue : NativeRNodeRadioConfig(), Structure.ByValue
}

@Structure.FieldOrder(
    "structSize",
    "name",
    "virtualPort",
    "radio",
    "flowControl",
    "outgoing",
)
internal open class NativeMultiRNodeMemberConfig : Structure() {
    @JvmField
    var structSize: SizeT = SizeT()

    @JvmField
    var name: NativeStringView.ByValue = NativeStringView.ByValue()

    @JvmField
    var virtualPort: Byte = 0

    @JvmField
    var radio: NativeRNodeRadioConfig.ByValue = NativeRNodeRadioConfig.ByValue()

    @JvmField
    var flowControl: Byte = 0

    @JvmField
    var outgoing: Byte = 0
}

@Structure.FieldOrder(
    "structSize",
    "kind",
    "hasGroupId",
    "groupId",
    "hasDiscoveryScope",
    "discoveryScope",
    "hasDiscoveryPort",
    "discoveryPort",
    "hasDataPort",
    "dataPort",
    "devices",
    "deviceCount",
    "ignoredDevices",
    "ignoredDeviceCount",
    "hasMulticastAddressType",
    "multicastAddressType",
    "target",
    "bind",
    "local",
    "peer",
    "bitrateKind",
    "bitrateBps",
    "port",
    "line",
    "flowControl",
    "preambleMillis",
    "transmitTailMillis",
    "persistence",
    "slotTimeMillis",
    "hasStationCallsign",
    "stationCallsign",
    "hasStationIntervalSeconds",
    "stationIntervalSeconds",
    "callsign",
    "ssid",
    "radio",
    "hasAirtimeLimitShortCentiPercent",
    "airtimeLimitShortCentiPercent",
    "hasAirtimeLimitLongCentiPercent",
    "airtimeLimitLongCentiPercent",
    "members",
    "memberCount",
    "command",
    "commandCount",
    "respawnDelayMillis",
    "peers",
    "peerCount",
    "connectable",
    "url",
)
internal class NativeInterfaceConfig : Structure() {
    @JvmField
    var structSize: SizeT = SizeT()

    @JvmField
    var kind: Int = 0

    @JvmField
    var hasGroupId: Byte = 0

    @JvmField
    var groupId: NativeStringView.ByValue = NativeStringView.ByValue()

    @JvmField
    var hasDiscoveryScope: Byte = 0

    @JvmField
    var discoveryScope: Int = 0

    @JvmField
    var hasDiscoveryPort: Byte = 0

    @JvmField
    var discoveryPort: Short = 0

    @JvmField
    var hasDataPort: Byte = 0

    @JvmField
    var dataPort: Short = 0

    @JvmField
    var devices: Pointer? = null

    @JvmField
    var deviceCount: SizeT = SizeT()

    @JvmField
    var ignoredDevices: Pointer? = null

    @JvmField
    var ignoredDeviceCount: SizeT = SizeT()

    @JvmField
    var hasMulticastAddressType: Byte = 0

    @JvmField
    var multicastAddressType: Int = 0

    @JvmField
    var target: NativeStringView.ByValue = NativeStringView.ByValue()

    @JvmField
    var bind: NativeStringView.ByValue = NativeStringView.ByValue()

    @JvmField
    var local: NativeStringView.ByValue = NativeStringView.ByValue()

    @JvmField
    var peer: NativeStringView.ByValue = NativeStringView.ByValue()

    @JvmField
    var bitrateKind: Int = 0

    @JvmField
    var bitrateBps: Long = 0

    @JvmField
    var port: NativeStringView.ByValue = NativeStringView.ByValue()

    @JvmField
    var line: NativeSerialLineConfig.ByValue = NativeSerialLineConfig.ByValue()

    @JvmField
    var flowControl: Byte = 0

    @JvmField
    var preambleMillis: Int = 0

    @JvmField
    var transmitTailMillis: Int = 0

    @JvmField
    var persistence: Byte = 0

    @JvmField
    var slotTimeMillis: Int = 0

    @JvmField
    var hasStationCallsign: Byte = 0

    @JvmField
    var stationCallsign: NativeStringView.ByValue = NativeStringView.ByValue()

    @JvmField
    var hasStationIntervalSeconds: Byte = 0

    @JvmField
    var stationIntervalSeconds: Long = 0

    @JvmField
    var callsign: NativeStringView.ByValue = NativeStringView.ByValue()

    @JvmField
    var ssid: Byte = 0

    @JvmField
    var radio: NativeRNodeRadioConfig.ByValue = NativeRNodeRadioConfig.ByValue()

    @JvmField
    var hasAirtimeLimitShortCentiPercent: Byte = 0

    @JvmField
    var airtimeLimitShortCentiPercent: Short = 0

    @JvmField
    var hasAirtimeLimitLongCentiPercent: Byte = 0

    @JvmField
    var airtimeLimitLongCentiPercent: Short = 0

    @JvmField
    var members: Pointer? = null

    @JvmField
    var memberCount: SizeT = SizeT()

    @JvmField
    var command: Pointer? = null

    @JvmField
    var commandCount: SizeT = SizeT()

    @JvmField
    var respawnDelayMillis: Long = 0

    @JvmField
    var peers: Pointer? = null

    @JvmField
    var peerCount: SizeT = SizeT()

    @JvmField
    var connectable: Byte = 0

    @JvmField
    var url: NativeStringView.ByValue = NativeStringView.ByValue()
}

@Structure.FieldOrder(
    "structSize",
    "hasMode",
    "mode",
    "hasGravity",
    "gravity",
    "hasRecursivePathRequests",
    "recursivePathRequests",
    "hasAnnouncesFromInternal",
    "announcesFromInternal",
    "hasAnnouncesToInternal",
    "announcesToInternal",
)
internal class NativeInterfaceRoutingPolicy : Structure() {
    @JvmField var structSize: SizeT = SizeT()
    @JvmField var hasMode: Byte = 0
    @JvmField var mode: Int = 0
    @JvmField var hasGravity: Byte = 0
    @JvmField var gravity: Long = 0
    @JvmField var hasRecursivePathRequests: Byte = 0
    @JvmField var recursivePathRequests: Byte = 0
    @JvmField var hasAnnouncesFromInternal: Byte = 0
    @JvmField var announcesFromInternal: Byte = 0
    @JvmField var hasAnnouncesToInternal: Byte = 0
    @JvmField var announcesToInternal: Byte = 0
}

internal fun NativeArena.interfaceConfig(value: InterfaceConfig): NativeInterfaceConfig {
    val result = NativeInterfaceConfig()
    result.structSize = SizeT(result.size().toLong())
    when (value) {
        is InterfaceConfigAutoLan -> {
            result.kind = InterfaceKind.AUTO_LAN.rawValue
            value.groupId?.let {
                result.hasGroupId = 1
                result.groupId = string(it)
            }
            value.discoveryScope?.let {
                result.hasDiscoveryScope = 1
                result.discoveryScope = it.rawValue
            }
            value.discoveryPort?.let {
                result.hasDiscoveryPort = 1
                result.discoveryPort = it.toShort()
            }
            value.dataPort?.let {
                result.hasDataPort = 1
                result.dataPort = it.toShort()
            }
            result.devices = stringArray(value.devices)
            result.deviceCount = SizeT(value.devices.size.toLong())
            result.ignoredDevices = stringArray(value.ignoredDevices)
            result.ignoredDeviceCount = SizeT(value.ignoredDevices.size.toLong())
            value.multicastAddressType?.let {
                result.hasMulticastAddressType = 1
                result.multicastAddressType = it.rawValue
            }
        }
        is InterfaceConfigTcpClient -> {
            result.kind = InterfaceKind.TCP_CLIENT.rawValue
            result.target = string(value.target)
            result.setBitrate(value.bitrate)
        }
        is InterfaceConfigTcpServer -> {
            result.kind = InterfaceKind.TCP_SERVER.rawValue
            result.bind = string(value.bind)
            result.setBitrate(value.bitrate)
        }
        is InterfaceConfigUdp -> {
            result.kind = InterfaceKind.UDP.rawValue
            result.local = string(value.local)
            result.peer = string(value.peer)
            result.setBitrate(value.bitrate)
        }
        is InterfaceConfigSerial -> {
            result.kind = InterfaceKind.SERIAL.rawValue
            result.port = string(value.port)
            result.line = serialLine(value.line)
        }
        is InterfaceConfigKiss -> {
            result.kind = InterfaceKind.KISS.rawValue
            result.port = string(value.port)
            result.line = serialLine(value.line)
            result.flowControl = value.flowControl.native()
            result.preambleMillis = value.preambleMillis.toInt()
            result.transmitTailMillis = value.transmitTailMillis.toInt()
            result.persistence = value.persistence.toByte()
            result.slotTimeMillis = value.slotTimeMillis.toInt()
            setStation(result, value.stationCallsign, value.stationIntervalSeconds)
        }
        is InterfaceConfigAx25Kiss -> {
            result.kind = InterfaceKind.AX25_KISS.rawValue
            result.port = string(value.port)
            result.line = serialLine(value.line)
            result.flowControl = value.flowControl.native()
            result.preambleMillis = value.preambleMillis.toInt()
            result.transmitTailMillis = value.transmitTailMillis.toInt()
            result.persistence = value.persistence.toByte()
            result.slotTimeMillis = value.slotTimeMillis.toInt()
            result.callsign = string(value.callsign)
            result.ssid = value.ssid.toByte()
        }
        is InterfaceConfigRNode -> {
            result.kind = InterfaceKind.R_NODE.rawValue
            result.port = string(value.port)
            result.radio = radio(value.radio)
            result.flowControl = value.flowControl.native()
            setStation(result, value.stationCallsign, value.stationIntervalSeconds)
            value.airtimeLimitShortCentiPercent?.let {
                result.hasAirtimeLimitShortCentiPercent = 1
                result.airtimeLimitShortCentiPercent = it.toShort()
            }
            value.airtimeLimitLongCentiPercent?.let {
                result.hasAirtimeLimitLongCentiPercent = 1
                result.airtimeLimitLongCentiPercent = it.toShort()
            }
        }
        is InterfaceConfigMultiRNode -> {
            result.kind = InterfaceKind.MULTI_R_NODE.rawValue
            result.port = string(value.port)
            setStation(result, value.stationCallsign, value.stationIntervalSeconds)
            result.members = structureArray(
                NativeMultiRNodeMemberConfig(),
                value.members.size,
            ) { target, index ->
                val member = value.members[index]
                target.structSize = SizeT(target.size().toLong())
                target.name = string(member.name)
                target.virtualPort = member.virtualPort.toByte()
                target.radio = radio(member.radio)
                target.flowControl = member.flowControl.native()
                target.outgoing = member.outgoing.native()
            }
            result.memberCount = SizeT(value.members.size.toLong())
        }
        is InterfaceConfigPipe -> {
            result.kind = InterfaceKind.PIPE.rawValue
            result.command = stringArray(value.command)
            result.commandCount = SizeT(value.command.size.toLong())
            result.respawnDelayMillis = value.respawnDelayMillis
        }
        is InterfaceConfigBackboneClient -> {
            result.kind = InterfaceKind.BACKBONE_CLIENT.rawValue
            result.target = string(value.target)
            result.setBitrate(value.bitrate)
        }
        is InterfaceConfigBackboneServer -> {
            result.kind = InterfaceKind.BACKBONE_SERVER.rawValue
            result.bind = string(value.bind)
            result.setBitrate(value.bitrate)
        }
        is InterfaceConfigI2p -> {
            result.kind = InterfaceKind.I2P.rawValue
            result.peers = stringArray(value.peers)
            result.peerCount = SizeT(value.peers.size.toLong())
            result.connectable = value.connectable.native()
        }
        is InterfaceConfigWeave -> {
            result.kind = InterfaceKind.WEAVE.rawValue
            result.port = string(value.port)
        }
        InterfaceConfigAutomaticUsb -> {
            result.kind = InterfaceKind.AUTOMATIC_USB.rawValue
        }
        InterfaceConfigAutomaticBluetoothLe -> {
            result.kind = InterfaceKind.AUTOMATIC_BLUETOOTH_LE.rawValue
        }
        is InterfaceConfigWebSocketClient -> {
            result.kind = InterfaceKind.WEB_SOCKET_CLIENT.rawValue
            result.target = string(value.target)
        }
        is InterfaceConfigWebSocketServer -> {
            result.kind = InterfaceKind.WEB_SOCKET_SERVER.rawValue
            result.bind = string(value.bind)
        }
        is InterfaceConfigBrowserRendezvous -> {
            result.kind = InterfaceKind.BROWSER_RENDEZVOUS.rawValue
            result.url = string(value.url)
        }
    }
    result.write()
    return result
}

internal fun interfaceRouting(value: InterfaceRoutingPolicy?): NativeInterfaceRoutingPolicy? =
    value?.let {
        require(it.gravity == null || it.gravity in HostContract.SAFE_INT_MIN..HostContract.SAFE_INT_MAX) {
            "gravity must be a safe integer"
        }
        NativeInterfaceRoutingPolicy().also { result ->
            result.structSize = SizeT(result.size().toLong())
            it.mode?.let { mode ->
                result.hasMode = 1
                result.mode = mode.rawValue
            }
            it.gravity?.let { gravity ->
                result.hasGravity = 1
                result.gravity = gravity
            }
            it.recursivePathRequests?.let { enabled ->
                result.hasRecursivePathRequests = 1
                result.recursivePathRequests = enabled.native()
            }
            it.announcesFromInternal?.let { enabled ->
                result.hasAnnouncesFromInternal = 1
                result.announcesFromInternal = enabled.native()
            }
            it.announcesToInternal?.let { enabled ->
                result.hasAnnouncesToInternal = 1
                result.announcesToInternal = enabled.native()
            }
            result.write()
        }
    }

private fun NativeArena.stringArray(values: List<String>): Pointer? =
    structureArray(NativeStringView(), values.size) { target, index ->
        val source = string(values[index])
        target.data = source.data
        target.length = source.length
    }

private fun NativeArena.serialLine(value: SerialLineConfig): NativeSerialLineConfig.ByValue =
    NativeSerialLineConfig.ByValue().also {
        it.structSize = SizeT(it.size().toLong())
        it.baud = value.baud.toInt()
        it.dataBits = value.dataBits.rawValue
        it.parity = value.parity.rawValue
        it.stopBits = value.stopBits.rawValue
        it.write()
    }

private fun NativeArena.radio(value: RNodeRadioConfig): NativeRNodeRadioConfig.ByValue =
    NativeRNodeRadioConfig.ByValue().also {
        it.structSize = SizeT(it.size().toLong())
        it.frequencyHz = value.frequencyHz
        it.bandwidthHz = value.bandwidthHz.toInt()
        it.txPowerDbm = value.txPowerDbm.toShort()
        it.spreadingFactor = value.spreadingFactor.toByte()
        it.codingRate = value.codingRate.toByte()
        it.write()
    }

private fun NativeInterfaceConfig.setBitrate(value: Bitrate) {
    val native = value.native()
    bitrateKind = native.first
    bitrateBps = native.second
}

private fun NativeArena.setStation(
    target: NativeInterfaceConfig,
    callsign: String?,
    intervalSeconds: Long?,
) {
    callsign?.let {
        target.hasStationCallsign = 1
        target.stationCallsign = string(it)
    }
    intervalSeconds?.let {
        target.hasStationIntervalSeconds = 1
        target.stationIntervalSeconds = it
    }
}

private fun Boolean.native(): Byte = if (this) 1 else 0
