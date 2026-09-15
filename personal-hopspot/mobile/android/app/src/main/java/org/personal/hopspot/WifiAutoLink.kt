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
import java.net.NetworkInterface
import java.nio.charset.StandardCharsets
import java.util.Collections
import java.util.LinkedHashMap
import java.util.LinkedHashSet

class WifiAutoLink(context: Context) {
    private val appContext = context.applicationContext
    private val multicastLock =
        (appContext.getSystemService(Context.WIFI_SERVICE) as? WifiManager)
            ?.createMulticastLock("PersonalHopspotAutoWifi")
            ?.apply { setReferenceCounted(false) }
    private val nsdManager = appContext.getSystemService(Context.NSD_SERVICE) as? NsdManager

    private val tcpServiceContract = ServiceContract(
        serviceType = NativeBridge.nativeWifiTcpServiceType(),
        port = validatedServicePort(
            "TCP",
            NativeBridge.nativeWifiTcpServicePort(),
        ),
    )
    private val udpServiceContract = ServiceContract(
        serviceType = NativeBridge.nativeWifiUdpServiceType(),
        port = validatedServicePort(
            "UDP",
            NativeBridge.nativeWifiUdpServicePort(),
        ),
    )
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
    private val resolvedCandidateInputCapacity =
        NativeBridge.nativeWifiResolvedCandidateInputCapacity().also { capacity ->
            require(capacity in candidateCapacity..UByte.MAX_VALUE.toInt()) {
                "Rust supplied an invalid resolved-candidate input capacity: $capacity"
            }
        }

    private val lifecycleMonitor = Any()
    private val discoveryMonitor = Any()
    private var linkLifecycle = LinkLifecycle.Stopped
    private var discoveryLifecycle: DiscoveryLifecycle = DiscoveryLifecycle.Stopped
    private var resolverState: ResolverState = ResolverState.Idle
    private var nextSessionId = 1L
    private var participationWorker: Thread? = null

    private val discoveredServices = LinkedHashSet<ServiceKey>()
    private val resolvedServices = LinkedHashSet<ServiceKey>()
    private val pendingResolutions = LinkedHashMap<ServiceKey, NsdServiceInfo>()

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
                "$candidateCapacity retained candidates per service",
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
        val publications = when (val publicationOutcome = centralPublications()) {
            is PublicationSessionOutcome.Ready -> publicationOutcome.publications
            PublicationSessionOutcome.NamesUnavailable -> {
                NativeBridge.nativeWifiEndPublicationSession()
                return DiscoveryTransitionOutcome.PublicationNamesUnavailable
            }
        }
        acquireMulticastLock()

        val sessionId = nextSessionId++
        val registrations = publications.map { publication ->
            ServiceRegistration(
                publication = publication,
                listener = registrationListener(sessionId, publication),
            )
        }
        val discoveries = publications.map { publication ->
            ServiceDiscoveryBrowse(
                contract = publication.contract,
                listener = discoveryListener(sessionId, publication.contract),
            )
        }
        val session = DiscoverySession(
            id = sessionId,
            manager = manager,
            registrations = registrations,
            discoveries = discoveries,
        )
        discoveryLifecycle = DiscoveryLifecycle.Active(session)

        return try {
            registrations.forEach { registration ->
                manager.registerService(
                    serviceInfo(registration.publication),
                    NsdManager.PROTOCOL_DNS_SD,
                    registration.listener,
                )
            }
            discoveries.forEach { discovery ->
                manager.discoverServices(
                    discovery.contract.serviceType,
                    NsdManager.PROTOCOL_DNS_SD,
                    discovery.listener,
                )
            }
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
                NativeBridge.nativeWifiEndPublicationSession()
                releaseMulticastLock()
                return DiscoveryTransitionOutcome.AlreadyInactive
            }
            is DiscoveryLifecycle.Active -> currentLifecycle.session
        }
        discoveryLifecycle = DiscoveryLifecycle.Stopped
        resolverState = ResolverState.Idle

        val removedServices = discoveredServices.toList()
        discoveredServices.clear()
        resolvedServices.clear()
        pendingResolutions.clear()
        removedServices.forEach { service ->
            NativeBridge.nativeWifiLost(service.contract.serviceType, service.instanceName)
        }
        NativeBridge.nativeWifiEndPublicationSession()

        nsdManager?.let { manager ->
            activeSession.discoveries.forEach { discovery ->
                try {
                    manager.stopServiceDiscovery(discovery.listener)
                } catch (failure: RuntimeException) {
                    Log.d(
                        TAG,
                        "${discovery.contract.serviceType} discovery was already stopped",
                        failure,
                    )
                }
            }
            activeSession.registrations.forEach { registration ->
                try {
                    manager.unregisterService(registration.listener)
                } catch (failure: RuntimeException) {
                    Log.d(
                        TAG,
                        "${registration.publication.contract.serviceType} advertisement was already unregistered",
                        failure,
                    )
                }
            }
        }
        releaseMulticastLock()
        return DiscoveryTransitionOutcome.Deactivated
    }

    private fun registrationListener(
        sessionId: Long,
        publication: DiscoveryPublication,
    ) =
        object : NsdManager.RegistrationListener {
            override fun onServiceRegistered(registeredService: NsdServiceInfo) {
                synchronized(discoveryMonitor) {
                    val session = activeSession(sessionId) ?: return
                    session.registrations
                        .firstOrNull { it.publication == publication }
                        ?.registeredName = registeredService.serviceName
                }
                NativeBridge.nativeWifiRegistered(
                    publication.contract.serviceType,
                    registeredService.serviceName,
                )
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

    private fun discoveryListener(sessionId: Long, serviceContract: ServiceContract) =
        object : NsdManager.DiscoveryListener {
            override fun onDiscoveryStarted(discoveredServiceType: String) {
                Log.i(TAG, "service discovery started for $discoveredServiceType")
            }

            override fun onServiceFound(service: NsdServiceInfo) {
                when (val admission = admitService(sessionId, serviceContract, service)) {
                    ServiceAdmission.Admitted,
                    ServiceAdmission.Updated,
                    -> {
                        Log.i(
                            TAG,
                            "service found (${admission.name}) ${service.serviceName} " +
                                serviceContract.serviceType,
                        )
                        pumpResolver()
                    }
                    ServiceAdmission.AtCapacity -> Log.w(
                        TAG,
                        "service discovery capacity reached; ignored ${service.serviceName}",
                    )
                    ServiceAdmission.OwnService -> Log.i(
                        TAG,
                        "service found (own) ${service.serviceName} ${serviceContract.serviceType}",
                    )
                    ServiceAdmission.StaleSession -> Log.d(
                        TAG,
                        "service found (stale) ${service.serviceName}",
                    )
                }
            }

            override fun onServiceLost(service: NsdServiceInfo) {
                val serviceKey = ServiceKey(serviceContract, service.serviceName)
                when (forgetService(sessionId, serviceKey)) {
                    ServiceRemoval.Removed -> {
                        Log.i(
                            TAG,
                            "service lost ${serviceKey.instanceName} ${serviceKey.contract.serviceType}",
                        )
                        NativeBridge.nativeWifiLost(
                            serviceKey.contract.serviceType,
                            serviceKey.instanceName,
                        )
                    }
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

    private fun admitService(
        sessionId: Long,
        serviceContract: ServiceContract,
        service: NsdServiceInfo,
    ): ServiceAdmission =
        synchronized(discoveryMonitor) {
            val session = activeSession(sessionId)
                ?: return@synchronized ServiceAdmission.StaleSession
            val serviceKey = ServiceKey(serviceContract, service.serviceName)
            if (session.isOwnService(serviceKey)) {
                return@synchronized ServiceAdmission.OwnService
            }
            if (discoveredServices.contains(serviceKey)) {
                pendingResolutions[serviceKey] = service
                return@synchronized ServiceAdmission.Updated
            }
            if (discoveredServices.size >= serviceCapacity) {
                return@synchronized ServiceAdmission.AtCapacity
            }
            discoveredServices.add(serviceKey)
            pendingResolutions[serviceKey] = service
            ServiceAdmission.Admitted
        }

    private fun forgetService(sessionId: Long, serviceKey: ServiceKey): ServiceRemoval =
        synchronized(discoveryMonitor) {
            if (activeSession(sessionId) == null) {
                return@synchronized ServiceRemoval.StaleSession
            }
            val wasDiscovered = discoveredServices.remove(serviceKey)
            val wasResolved = resolvedServices.remove(serviceKey)
            val wasPending = pendingResolutions.remove(serviceKey) != null
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
                        Log.i(
                            TAG,
                            "resolving ${work.serviceKey.instanceName} " +
                                work.serviceKey.contract.serviceType,
                        )
                        work.manager.resolveService(
                            work.service,
                            resolutionListener(work.sessionId, work.serviceKey),
                        )
                        return
                    } catch (failure: RuntimeException) {
                        Log.w(TAG, "could not resolve ${work.serviceKey.instanceName}", failure)
                        completeResolution(work.sessionId, work.serviceKey)
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
            serviceKey = nextService.key,
            service = nextService.value,
        )
    }

    private fun resolutionListener(sessionId: Long, serviceKey: ServiceKey) =
        object : NsdManager.ResolveListener {
            override fun onServiceResolved(service: NsdServiceInfo) {
                when (completeResolution(sessionId, serviceKey)) {
                    ResolutionCompletion.VisibleService -> publishResolvedService(
                        sessionId,
                        serviceKey,
                        service,
                    )
                    ResolutionCompletion.RemovedService,
                    ResolutionCompletion.StaleResolution,
                    -> Unit
                }
                pumpResolver()
            }

            override fun onResolveFailed(service: NsdServiceInfo, errorCode: Int) {
                Log.w(
                    TAG,
                    "service resolve failed ${serviceKey.instanceName} " +
                        "${serviceKey.contract.serviceType} code=$errorCode",
                )
                completeResolution(sessionId, serviceKey)
                pumpResolver()
            }
        }

    private fun completeResolution(
        sessionId: Long,
        serviceKey: ServiceKey,
    ): ResolutionCompletion = synchronized(discoveryMonitor) {
        val expectedResolution = ResolverState.Resolving(sessionId, serviceKey)
        if (resolverState != expectedResolution) {
            return@synchronized ResolutionCompletion.StaleResolution
        }
        resolverState = ResolverState.Idle
        if (activeSession(sessionId) == null) {
            ResolutionCompletion.StaleResolution
        } else if (discoveredServices.contains(serviceKey)) {
            ResolutionCompletion.VisibleService
        } else {
            ResolutionCompletion.RemovedService
        }
    }

    private fun publishResolvedService(
        sessionId: Long,
        serviceKey: ServiceKey,
        service: NsdServiceInfo,
    ): ResolvedServicePublicationOutcome = synchronized(discoveryMonitor) {
        if (activeSession(sessionId) == null || !discoveredServices.contains(serviceKey)) {
            return@synchronized ResolvedServicePublicationOutcome.StaleSession
        }
        val version = serviceVersion(service)
        val candidates = resolvedCandidates(service)
        val publication = ResolvedServicePublicationOutcome.fromBridge(
            NativeBridge.nativeWifiResolved(
                serviceKey.contract.serviceType,
                serviceKey.instanceName,
                candidates.map(ResolvedCandidate::octets).toTypedArray(),
                candidates.map(ResolvedCandidate::scopeId).toIntArray(),
                service.port,
                version,
            ),
        )
        when (publication) {
            ResolvedServicePublicationOutcome.Visible -> {
                resolvedServices.add(serviceKey)
                Log.i(
                    TAG,
                    "resolved ${serviceKey.instanceName} ${serviceKey.contract.serviceType} " +
                        "candidates=${candidates.size} scopes=${candidates.map { it.scopeId }}",
                )
            }
            ResolvedServicePublicationOutcome.Rejected,
            ResolvedServicePublicationOutcome.AtCapacity,
            ResolvedServicePublicationOutcome.Unavailable,
            ResolvedServicePublicationOutcome.Unrecognized,
            ResolvedServicePublicationOutcome.StaleSession,
            -> {
                resolvedServices.remove(serviceKey)
                Log.w(
                    TAG,
                    "resolve rejected (${publication.name}) ${serviceKey.instanceName} " +
                        "${serviceKey.contract.serviceType} candidates=${candidates.size} " +
                        "scopes=${candidates.map { it.scopeId }}",
                )
            }
        }
        publication
    }

    private fun centralPublications(): PublicationSessionOutcome {
        val tcpPublicationName = NativeBridge.nativeWifiTcpPublicationName()
            ?: return PublicationSessionOutcome.NamesUnavailable
        val udpPublicationName = NativeBridge.nativeWifiUdpPublicationName()
            ?: return PublicationSessionOutcome.NamesUnavailable
        return PublicationSessionOutcome.Ready(
            listOf(
                DiscoveryPublication(tcpServiceContract, tcpPublicationName),
                DiscoveryPublication(udpServiceContract, udpPublicationName),
            ),
        )
    }

    private fun serviceInfo(publication: DiscoveryPublication) = NsdServiceInfo().apply {
        serviceName = publication.instanceName
        serviceType = publication.contract.serviceType
        port = publication.contract.port
        when (AndroidDiscoveryVersionMetadata.forApiLevel(Build.VERSION.SDK_INT)) {
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

    /**
     * Build resolve candidates for Rust. UDP peering requires scoped IPv6 link-local
     * endpoints; NsdManager often returns `fe80::` with [Inet6Address.scopeId] == 0
     * (same class of bug the tokio/macOS mDNS path fills from local NIC ifindexes).
     */
    private fun resolvedCandidates(service: NsdServiceInfo): List<ResolvedCandidate> {
        val fallbackScopes = fallbackLinkLocalScopeIds()
        val candidates = ArrayList<ResolvedCandidate>(resolvedCandidateInputCapacity)
        for (address in serviceAddresses(service)) {
            when (address) {
                is Inet4Address -> {
                    candidates.add(ResolvedCandidate(address.address, 0))
                }
                is Inet6Address -> {
                    if (address.isLinkLocalAddress) {
                        val reportedScope = address.scopeId
                        val scopes = if (reportedScope != 0) {
                            listOf(reportedScope)
                        } else {
                            fallbackScopes
                        }
                        if (reportedScope == 0) {
                            if (scopes.isEmpty()) {
                                Log.w(
                                    TAG,
                                    "mDNS link-local missing scope and no local Wi-Fi ifindex; dropping",
                                )
                            } else {
                                Log.i(
                                    TAG,
                                    "mDNS link-local missing scope; using ifindex ${scopes.joinToString()}",
                                )
                            }
                        }
                        for (scopeId in scopes) {
                            if (scopeId == 0) {
                                continue
                            }
                            candidates.add(ResolvedCandidate(address.address, scopeId))
                            if (candidates.size >= resolvedCandidateInputCapacity) {
                                return candidates
                            }
                        }
                    } else {
                        candidates.add(ResolvedCandidate(address.address, 0))
                    }
                }
                else -> Unit
            }
            if (candidates.size >= resolvedCandidateInputCapacity) {
                break
            }
        }
        return candidates
    }

    /** Local AutoWifi NIC ifindexes used when a resolved AAAA omits IPv6 scope. */
    private fun fallbackLinkLocalScopeIds(): List<Int> {
        val scopes = LinkedHashSet<Int>()
        try {
            NetworkInterface.getByName("wlan0")
                ?.takeIf { nif -> nif.isUp && !nif.isLoopback && nif.index > 0 }
                ?.let { scopes.add(it.index) }
        } catch (_: Exception) {
            // Best-effort; continue with a broader scan.
        }
        try {
            for (nif in Collections.list(NetworkInterface.getNetworkInterfaces())) {
                if (!nif.isUp || nif.isLoopback || nif.index <= 0) {
                    continue
                }
                val hasLinkLocal = Collections.list(nif.inetAddresses).any { address ->
                    address is Inet6Address && address.isLinkLocalAddress
                }
                if (hasLinkLocal) {
                    scopes.add(nif.index)
                }
            }
        } catch (_: Exception) {
            // Best-effort; empty means LL candidates without scope are dropped.
        }
        return scopes.toList()
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
        val registrations: List<ServiceRegistration>,
        val discoveries: List<ServiceDiscoveryBrowse>,
    ) {
        fun isOwnService(serviceKey: ServiceKey): Boolean = registrations.any { registration ->
            registration.publication.contract == serviceKey.contract &&
                (
                    registration.publication.instanceName.equals(
                        serviceKey.instanceName,
                        ignoreCase = true,
                    ) || registration.registeredName?.equals(
                        serviceKey.instanceName,
                        ignoreCase = true,
                    ) == true
                )
        }
    }

    private data class ServiceContract(
        val serviceType: String,
        val port: Int,
    )

    private data class DiscoveryPublication(
        val contract: ServiceContract,
        val instanceName: String,
    )

    private data class ServiceRegistration(
        val publication: DiscoveryPublication,
        val listener: NsdManager.RegistrationListener,
        var registeredName: String? = null,
    )

    private data class ServiceDiscoveryBrowse(
        val contract: ServiceContract,
        val listener: NsdManager.DiscoveryListener,
    )

    private data class ServiceKey(
        val contract: ServiceContract,
        val instanceName: String,
    )

    private sealed class PublicationSessionOutcome {
        data class Ready(
            val publications: List<DiscoveryPublication>,
        ) : PublicationSessionOutcome()

        object NamesUnavailable : PublicationSessionOutcome()
    }

    private sealed class ResolverState {
        object Idle : ResolverState()
        data class Resolving(val sessionId: Long, val serviceKey: ServiceKey) : ResolverState()
    }

    private sealed class ResolutionWork {
        object None : ResolutionWork()
        data class Resolve(
            val manager: NsdManager,
            val sessionId: Long,
            val serviceKey: ServiceKey,
            val service: NsdServiceInfo,
        ) : ResolutionWork()
    }

    private enum class DiscoveryTransitionOutcome {
        Activated,
        AlreadyActive,
        Deactivated,
        AlreadyInactive,
        NsdUnavailable,
        PublicationNamesUnavailable,
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

    private enum class ResolvedServicePublicationOutcome(val bridgeValue: Int?) {
        Visible(NativeBridge.WIFI_RESOLVED_SERVICE_VISIBLE),
        Rejected(NativeBridge.WIFI_RESOLVED_SERVICE_REJECTED),
        AtCapacity(NativeBridge.WIFI_RESOLVED_SERVICE_AT_CAPACITY),
        Unavailable(NativeBridge.WIFI_RESOLVED_SERVICE_UNAVAILABLE),
        Unrecognized(null),
        StaleSession(null);

        companion object {
            fun fromBridge(bridgeValue: Int): ResolvedServicePublicationOutcome =
                values().firstOrNull { it.bridgeValue == bridgeValue } ?: Unrecognized
        }
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
        private const val PARTICIPATION_RETRY_MILLIS = 1_000L
        private const val WORKER_JOIN_MILLIS = 2_000L

        private fun validatedServicePort(transportName: String, port: Int): Int {
            require(port in 1..UShort.MAX_VALUE.toInt()) {
                "Rust supplied an invalid Android $transportName service port: $port"
            }
            return port
        }
    }
}
