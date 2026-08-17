package org.personal.hopspot

import android.content.Context
import android.net.nsd.NsdManager
import android.net.nsd.NsdServiceInfo
import android.net.wifi.WifiManager
import android.os.Build
import android.util.Log
import java.net.Inet4Address
import java.net.Inet6Address
import java.net.InetAddress
import java.nio.charset.StandardCharsets
import java.util.LinkedHashMap
import java.util.LinkedHashSet

class WifiAutoLink(context: Context) {
    private val appContext = context.applicationContext
    private val multicastLock =
        (appContext.getSystemService(Context.WIFI_SERVICE) as? WifiManager)
            ?.createMulticastLock("PersonalHopspotAutoWifi")
            ?.apply { setReferenceCounted(false) }
    private val nsdManager = appContext.getSystemService(Context.NSD_SERVICE) as? NsdManager

    private val serviceType = NativeBridge.nativeWifiServiceType()
    private val versionKey = NativeBridge.nativeWifiTxtVersionKey()
    private val versionValue = NativeBridge.nativeWifiTxtVersionValue()
    private val serviceCapacity =
        NativeBridge.nativeWifiServiceCapacity().also { capacity ->
            require(capacity in 1..UByte.MAX_VALUE.toInt()) {
                "Rust supplied an invalid Android service-discovery capacity: $capacity"
            }
        }
    private val candidateCapacity =
        NativeBridge.nativeWifiCandidateCapacity().also { capacity ->
            require(capacity in 1..UByte.MAX_VALUE.toInt()) {
                "Rust supplied an invalid service-advertisement candidate capacity: $capacity"
            }
        }

    private val lifecycleMonitor = Any()
    private val discoveryMonitor = Any()
    private var linkLifecycle = LinkLifecycle.Stopped
    private var discoveryLifecycle: DiscoveryLifecycle = DiscoveryLifecycle.Stopped
    private var resolverState: ResolverState = ResolverState.Idle
    private var nextSessionId = 1L
    private var participationWorker: Thread? = null

    private val discoveredServices = LinkedHashSet<String>()
    private val resolvedServices = LinkedHashSet<String>()
    private val pendingResolutions = LinkedHashMap<String, NsdServiceInfo>()

    fun start() {
        val worker = synchronized(lifecycleMonitor) {
            when (linkLifecycle) {
                LinkLifecycle.Running -> return
                LinkLifecycle.Stopped -> {
                    linkLifecycle = LinkLifecycle.Running
                    Thread(::runParticipationPump, "prns-wifi-discovery").also {
                        participationWorker = it
                    }
                }
            }
        }
        Log.i(
            TAG,
            "service discovery bounded to $serviceCapacity services and " +
                "$candidateCapacity candidates per service",
        )
        worker.start()
    }

    fun stop() {
        val worker = synchronized(lifecycleMonitor) {
            when (linkLifecycle) {
                LinkLifecycle.Stopped -> null
                LinkLifecycle.Running -> {
                    linkLifecycle = LinkLifecycle.Stopped
                    participationWorker.also { participationWorker = null }
                }
            }
        }
        NativeBridge.nativeWifiWakeDiscoveryPump()
        applyParticipation(DiscoveryParticipation.Inactive)
        if (worker != null && worker !== Thread.currentThread()) {
            try {
                worker.join(WORKER_JOIN_MILLIS)
            } catch (interrupted: InterruptedException) {
                Thread.currentThread().interrupt()
                Log.d(TAG, "interrupted while joining service-discovery worker", interrupted)
            }
        }
    }

    private fun runParticipationPump() {
        var observedGeneration = NativeBridge.nativeWifiWorkGeneration()
        while (currentLinkLifecycle() == LinkLifecycle.Running) {
            val requestedParticipation = DiscoveryParticipation.fromBridge(
                NativeBridge.nativeWifiDiscoveryParticipation(),
            )
            val transitionOutcome = applyParticipation(requestedParticipation)
            if (transitionOutcome == DiscoveryTransitionOutcome.UnrecognizedParticipation) {
                Log.w(TAG, "Rust reported an unrecognized service-discovery participation state")
            }
            observedGeneration = NativeBridge.nativeWifiWaitForWork(
                observedGeneration,
                PARTICIPATION_RETRY_MILLIS,
            )
        }
        applyParticipation(DiscoveryParticipation.Inactive)
    }

    private fun currentLinkLifecycle(): LinkLifecycle =
        synchronized(lifecycleMonitor) { linkLifecycle }

    private fun applyParticipation(
        requestedParticipation: DiscoveryParticipation,
    ): DiscoveryTransitionOutcome = synchronized(discoveryMonitor) {
        val effectiveParticipation = when (currentLinkLifecycle()) {
            LinkLifecycle.Running -> requestedParticipation
            LinkLifecycle.Stopped -> DiscoveryParticipation.Inactive
        }
        when (effectiveParticipation) {
            DiscoveryParticipation.Central -> activateDiscovery()
            DiscoveryParticipation.Inactive,
            DiscoveryParticipation.Satellite,
            -> deactivateDiscovery()
            DiscoveryParticipation.Unrecognized -> {
                deactivateDiscovery()
                DiscoveryTransitionOutcome.UnrecognizedParticipation
            }
        }
    }

    private fun activateDiscovery(): DiscoveryTransitionOutcome {
        if (discoveryLifecycle is DiscoveryLifecycle.Active) {
            return DiscoveryTransitionOutcome.AlreadyActive
        }
        val manager = nsdManager ?: return DiscoveryTransitionOutcome.NsdUnavailable
        acquireMulticastLock()

        val sessionId = nextSessionId++
        val registrationListener = registrationListener(sessionId)
        val discoveryListener = discoveryListener(sessionId)
        val session = DiscoverySession(
            id = sessionId,
            manager = manager,
            registrationListener = registrationListener,
            discoveryListener = discoveryListener,
        )
        discoveryLifecycle = DiscoveryLifecycle.Active(session)

        val serviceInfo = NsdServiceInfo().apply {
            serviceName = SERVICE_NAME
            serviceType = this@WifiAutoLink.serviceType
            port = NativeBridge.nativeWifiServicePort()
            when (
                AndroidDiscoveryVersionMetadata.forApiLevel(Build.VERSION.SDK_INT)
            ) {
                AndroidDiscoveryVersionMetadata.ImplicitV1 -> Unit
                AndroidDiscoveryVersionMetadata.ExplicitV1 -> {
                    if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.LOLLIPOP) {
                        setAttribute(versionKey, versionValue)
                    } else {
                        error("explicit DNS-SD version metadata requires Android API 21")
                    }
                }
            }
        }
        return try {
            manager.registerService(
                serviceInfo,
                NsdManager.PROTOCOL_DNS_SD,
                registrationListener,
            )
            manager.discoverServices(
                serviceType,
                NsdManager.PROTOCOL_DNS_SD,
                discoveryListener,
            )
            DiscoveryTransitionOutcome.Activated
        } catch (failure: RuntimeException) {
            Log.w(TAG, "could not start Android service discovery", failure)
            deactivateDiscovery()
            DiscoveryTransitionOutcome.ActivationFailed
        }
    }

    private fun deactivateDiscovery(): DiscoveryTransitionOutcome {
        val activeSession = when (val currentLifecycle = discoveryLifecycle) {
            DiscoveryLifecycle.Stopped -> {
                releaseMulticastLock()
                return DiscoveryTransitionOutcome.AlreadyInactive
            }
            is DiscoveryLifecycle.Active -> currentLifecycle.session
        }
        discoveryLifecycle = DiscoveryLifecycle.Stopped
        resolverState = ResolverState.Idle

        val removedServiceNames = discoveredServices.toList()
        discoveredServices.clear()
        resolvedServices.clear()
        pendingResolutions.clear()
        removedServiceNames.forEach(NativeBridge::nativeWifiLost)

        nsdManager?.let { manager ->
            try {
                manager.stopServiceDiscovery(activeSession.discoveryListener)
            } catch (failure: RuntimeException) {
                Log.d(TAG, "service discovery was already stopped", failure)
            }
            try {
                manager.unregisterService(activeSession.registrationListener)
            } catch (failure: RuntimeException) {
                Log.d(TAG, "service advertisement was already unregistered", failure)
            }
        }
        releaseMulticastLock()
        return DiscoveryTransitionOutcome.Deactivated
    }

    private fun registrationListener(sessionId: Long) =
        object : NsdManager.RegistrationListener {
            override fun onServiceRegistered(registeredService: NsdServiceInfo) {
                synchronized(discoveryMonitor) {
                    val session = activeSession(sessionId) ?: return
                    session.registeredName = registeredService.serviceName
                }
                Log.i(
                    TAG,
                    "service discovery registered ${registeredService.serviceName} " +
                        "on :${registeredService.port}",
                )
            }

            override fun onRegistrationFailed(service: NsdServiceInfo, errorCode: Int) {
                handleSessionFailure(sessionId, "service registration", errorCode)
            }

            override fun onServiceUnregistered(service: NsdServiceInfo) {
                handleSessionEnded(
                    sessionId,
                    "service advertisement ${service.serviceName} was unregistered",
                )
            }

            override fun onUnregistrationFailed(service: NsdServiceInfo, errorCode: Int) {
                Log.d(TAG, "service unregistration failed code=$errorCode")
            }
        }

    private fun discoveryListener(sessionId: Long) =
        object : NsdManager.DiscoveryListener {
            override fun onDiscoveryStarted(discoveredServiceType: String) {
                Log.i(TAG, "service discovery started for $discoveredServiceType")
            }

            override fun onServiceFound(service: NsdServiceInfo) {
                when (admitService(sessionId, service)) {
                    ServiceAdmission.Admitted,
                    ServiceAdmission.Updated,
                    -> pumpResolver()
                    ServiceAdmission.AtCapacity -> Log.w(
                        TAG,
                        "service discovery capacity reached; ignored ${service.serviceName}",
                    )
                    ServiceAdmission.OwnService,
                    ServiceAdmission.StaleSession,
                    -> Unit
                }
            }

            override fun onServiceLost(service: NsdServiceInfo) {
                when (forgetService(sessionId, service.serviceName)) {
                    ServiceRemoval.Removed -> NativeBridge.nativeWifiLost(service.serviceName)
                    ServiceRemoval.NotPresent,
                    ServiceRemoval.StaleSession,
                    -> Unit
                }
            }

            override fun onDiscoveryStopped(discoveredServiceType: String) {
                handleSessionEnded(
                    sessionId,
                    "service discovery stopped for $discoveredServiceType",
                )
            }

            override fun onStartDiscoveryFailed(discoveredServiceType: String, errorCode: Int) {
                handleSessionFailure(sessionId, "service discovery start", errorCode)
            }

            override fun onStopDiscoveryFailed(discoveredServiceType: String, errorCode: Int) {
                Log.d(TAG, "service discovery stop failed code=$errorCode")
            }
        }

    private fun activeSession(sessionId: Long): DiscoverySession? =
        when (val currentLifecycle = discoveryLifecycle) {
            DiscoveryLifecycle.Stopped -> null
            is DiscoveryLifecycle.Active -> currentLifecycle.session.takeIf { it.id == sessionId }
        }

    private fun handleSessionFailure(sessionId: Long, operation: String, errorCode: Int) {
        synchronized(discoveryMonitor) {
            when (activeSession(sessionId)) {
                null -> Unit
                else -> {
                    Log.w(TAG, "$operation failed code=$errorCode")
                    deactivateDiscovery()
                }
            }
        }
    }

    private fun handleSessionEnded(sessionId: Long, reason: String) {
        synchronized(discoveryMonitor) {
            when (activeSession(sessionId)) {
                null -> Unit
                else -> {
                    Log.w(TAG, reason)
                    deactivateDiscovery()
                }
            }
        }
    }

    private fun admitService(sessionId: Long, service: NsdServiceInfo): ServiceAdmission =
        synchronized(discoveryMonitor) {
            val session = activeSession(sessionId) ?: return@synchronized ServiceAdmission.StaleSession
            val serviceName = service.serviceName
            if (serviceName == session.registeredName) {
                return@synchronized ServiceAdmission.OwnService
            }
            if (discoveredServices.contains(serviceName)) {
                pendingResolutions[serviceName] = service
                return@synchronized ServiceAdmission.Updated
            }
            if (discoveredServices.size >= serviceCapacity) {
                return@synchronized ServiceAdmission.AtCapacity
            }
            discoveredServices.add(serviceName)
            pendingResolutions[serviceName] = service
            ServiceAdmission.Admitted
        }

    private fun forgetService(sessionId: Long, serviceName: String): ServiceRemoval =
        synchronized(discoveryMonitor) {
            if (activeSession(sessionId) == null) {
                return@synchronized ServiceRemoval.StaleSession
            }
            val wasDiscovered = discoveredServices.remove(serviceName)
            val wasResolved = resolvedServices.remove(serviceName)
            val wasPending = pendingResolutions.remove(serviceName) != null
            if (wasDiscovered || wasResolved || wasPending) {
                ServiceRemoval.Removed
            } else {
                ServiceRemoval.NotPresent
            }
        }

    @Suppress("DEPRECATION")
    private fun pumpResolver() {
        while (true) {
            when (val work = takeResolutionWork()) {
                ResolutionWork.None -> return
                is ResolutionWork.Resolve -> {
                    try {
                        work.manager.resolveService(
                            work.service,
                            resolutionListener(work.sessionId, work.serviceName),
                        )
                        return
                    } catch (failure: RuntimeException) {
                        Log.d(TAG, "could not resolve ${work.serviceName}", failure)
                        completeResolution(work.sessionId, work.serviceName)
                    }
                }
            }
        }
    }

    private fun takeResolutionWork(): ResolutionWork = synchronized(discoveryMonitor) {
        val activeSession = when (val currentLifecycle = discoveryLifecycle) {
            DiscoveryLifecycle.Stopped -> return@synchronized ResolutionWork.None
            is DiscoveryLifecycle.Active -> currentLifecycle.session
        }
        if (resolverState is ResolverState.Resolving) {
            return@synchronized ResolutionWork.None
        }
        val pending = pendingResolutions.entries.iterator()
        if (!pending.hasNext()) {
            return@synchronized ResolutionWork.None
        }
        val nextService = pending.next()
        pending.remove()
        resolverState = ResolverState.Resolving(activeSession.id, nextService.key)
        ResolutionWork.Resolve(
            manager = activeSession.manager,
            sessionId = activeSession.id,
            serviceName = nextService.key,
            service = nextService.value,
        )
    }

    private fun resolutionListener(sessionId: Long, serviceName: String) =
        object : NsdManager.ResolveListener {
            override fun onServiceResolved(service: NsdServiceInfo) {
                when (completeResolution(sessionId, serviceName)) {
                    ResolutionCompletion.VisibleService -> publishResolvedService(
                        sessionId,
                        serviceName,
                        service,
                    )
                    ResolutionCompletion.RemovedService,
                    ResolutionCompletion.StaleResolution,
                    -> Unit
                }
                pumpResolver()
            }

            override fun onResolveFailed(service: NsdServiceInfo, errorCode: Int) {
                Log.d(TAG, "service resolve failed code=$errorCode")
                completeResolution(sessionId, serviceName)
                pumpResolver()
            }
        }

    private fun completeResolution(
        sessionId: Long,
        serviceName: String,
    ): ResolutionCompletion = synchronized(discoveryMonitor) {
        val expectedResolution = ResolverState.Resolving(sessionId, serviceName)
        if (resolverState != expectedResolution) {
            return@synchronized ResolutionCompletion.StaleResolution
        }
        resolverState = ResolverState.Idle
        if (activeSession(sessionId) == null) {
            ResolutionCompletion.StaleResolution
        } else if (discoveredServices.contains(serviceName)) {
            ResolutionCompletion.VisibleService
        } else {
            ResolutionCompletion.RemovedService
        }
    }

    private fun publishResolvedService(
        sessionId: Long,
        serviceName: String,
        service: NsdServiceInfo,
    ): ServicePublication = synchronized(discoveryMonitor) {
        if (activeSession(sessionId) == null || !discoveredServices.contains(serviceName)) {
            return@synchronized ServicePublication.StaleSession
        }
        val version = serviceVersion(service)
        val candidates = serviceAddresses(service)
            .asSequence()
            .mapNotNull(::resolvedCandidate)
            .take(candidateCapacity)
            .toList()
        val publication = if (
            NativeBridge.nativeWifiResolved(
                serviceName,
                candidates.map(ResolvedCandidate::octets).toTypedArray(),
                candidates.map(ResolvedCandidate::scopeId).toIntArray(),
                service.port,
                version,
            )
        ) {
            ServicePublication.Published
        } else {
            ServicePublication.Rejected
        }
        if (publication == ServicePublication.Published) {
            resolvedServices.add(serviceName)
        } else {
            resolvedServices.remove(serviceName)
        }
        publication
    }

    private fun resolvedCandidate(address: InetAddress): ResolvedCandidate? =
        when (address) {
            is Inet4Address -> ResolvedCandidate(address.address, 0)
            is Inet6Address -> ResolvedCandidate(address.address, address.scopeId)
            else -> null
        }

    @Suppress("DEPRECATION")
    private fun serviceAddresses(service: NsdServiceInfo): List<InetAddress> =
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.UPSIDE_DOWN_CAKE) {
            service.hostAddresses
        } else {
            listOfNotNull(service.host)
        }

    private fun serviceVersion(service: NsdServiceInfo): String? = when (
        AndroidDiscoveryVersionMetadata.forApiLevel(Build.VERSION.SDK_INT)
    ) {
        AndroidDiscoveryVersionMetadata.ImplicitV1 -> null
        AndroidDiscoveryVersionMetadata.ExplicitV1 -> {
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.LOLLIPOP) {
                service.attributes[versionKey]?.toString(StandardCharsets.UTF_8)
            } else {
                error("explicit DNS-SD version metadata requires Android API 21")
            }
        }
    }

    private fun acquireMulticastLock(): MulticastLockOutcome {
        val currentLock = multicastLock ?: return MulticastLockOutcome.Unavailable
        if (currentLock.isHeld) {
            return MulticastLockOutcome.AlreadyHeld
        }
        return try {
            currentLock.acquire()
            Log.i(TAG, "wifi multicast lock acquired")
            MulticastLockOutcome.Acquired
        } catch (failure: RuntimeException) {
            Log.w(TAG, "wifi multicast lock unavailable", failure)
            MulticastLockOutcome.AcquisitionFailed
        }
    }

    private fun releaseMulticastLock(): MulticastLockOutcome {
        val currentLock = multicastLock ?: return MulticastLockOutcome.Unavailable
        if (!currentLock.isHeld) {
            return MulticastLockOutcome.AlreadyReleased
        }
        return try {
            currentLock.release()
            Log.i(TAG, "wifi multicast lock released")
            MulticastLockOutcome.Released
        } catch (failure: RuntimeException) {
            Log.w(TAG, "could not release wifi multicast lock", failure)
            MulticastLockOutcome.ReleaseFailed
        }
    }

    private enum class LinkLifecycle {
        Stopped,
        Running,
    }

    private enum class DiscoveryParticipation(val bridgeValue: Int?) {
        Inactive(NativeBridge.WIFI_DISCOVERY_INACTIVE),
        Satellite(NativeBridge.WIFI_DISCOVERY_SATELLITE),
        Central(NativeBridge.WIFI_DISCOVERY_CENTRAL),
        Unrecognized(null);

        companion object {
            fun fromBridge(bridgeValue: Int): DiscoveryParticipation =
                values().firstOrNull { it.bridgeValue == bridgeValue } ?: Unrecognized
        }
    }

    private sealed class DiscoveryLifecycle {
        object Stopped : DiscoveryLifecycle()
        data class Active(val session: DiscoverySession) : DiscoveryLifecycle()
    }

    private data class DiscoverySession(
        val id: Long,
        val manager: NsdManager,
        val registrationListener: NsdManager.RegistrationListener,
        val discoveryListener: NsdManager.DiscoveryListener,
        var registeredName: String? = null,
    )

    private sealed class ResolverState {
        object Idle : ResolverState()
        data class Resolving(val sessionId: Long, val serviceName: String) : ResolverState()
    }

    private sealed class ResolutionWork {
        object None : ResolutionWork()
        data class Resolve(
            val manager: NsdManager,
            val sessionId: Long,
            val serviceName: String,
            val service: NsdServiceInfo,
        ) : ResolutionWork()
    }

    private enum class DiscoveryTransitionOutcome {
        Activated,
        AlreadyActive,
        Deactivated,
        AlreadyInactive,
        NsdUnavailable,
        ActivationFailed,
        UnrecognizedParticipation,
    }

    private enum class ServiceAdmission {
        Admitted,
        Updated,
        OwnService,
        AtCapacity,
        StaleSession,
    }

    private enum class ServiceRemoval {
        Removed,
        NotPresent,
        StaleSession,
    }

    private enum class ResolutionCompletion {
        VisibleService,
        RemovedService,
        StaleResolution,
    }

    private enum class ServicePublication {
        Published,
        Rejected,
        StaleSession,
    }

    private data class ResolvedCandidate(
        val octets: ByteArray,
        val scopeId: Int,
    )

    private enum class MulticastLockOutcome {
        Acquired,
        AlreadyHeld,
        Released,
        AlreadyReleased,
        Unavailable,
        AcquisitionFailed,
        ReleaseFailed,
    }

    private companion object {
        private const val TAG = "HopspotWifi"
        private const val SERVICE_NAME = "PersonalHopspot"
        private const val PARTICIPATION_RETRY_MILLIS = 1_000L
        private const val WORKER_JOIN_MILLIS = 2_000L
    }
}
