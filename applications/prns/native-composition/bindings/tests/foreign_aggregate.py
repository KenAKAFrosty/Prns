"""Exercise app and SDK namespaces from one real aggregate native image.

Generate Python bindings from libprns_app and put the image next to them. Pass
that directory here. This tests foreign ownership and storage preservation;
mobile runtimes and OS background behavior require separate device tests.
"""
import asyncio
import gc
from pathlib import Path
import sys
import tempfile

sys.path.insert(0, sys.argv[1])
import importlib
package = Path(sys.argv[1]).resolve()
sys.path.insert(0, str(package.parent))
app = importlib.import_module(f"{package.name}.prns_app")
sdk = importlib.import_module(f"{package.name}.prns_host_uniffi")


async def run():
    with tempfile.TemporaryDirectory(prefix="prns-aggregate-") as directory:
        root = str(Path(directory) / "prns" / "development")
        assert isinstance(app.native_prepare_storage(root), app.NativeStoragePreparationOutcome.PREPARED)
        created = app.native_create_generated_identity(root)
        assert isinstance(created, app.IdentityCreationOutcome.CREATED), created
        identity = created.identity_hash
        for _ in range(2):
            started = app.native_start(root, app.DevelopmentNodeStartInput(development_tcp_target=None))
            assert isinstance(started, app.DevelopmentNodeStartOutcome.STARTED), started
            try:
                borrowed = app.shared_host()
                assert isinstance(borrowed, sdk.HostClientHandle)
                assert borrowed.identity_hash() == identity
                host_snapshot = await borrowed.snapshot()
                assert host_snapshot.persistence.restored
                assert host_snapshot.runtime.running
                product = await app.read_snapshot()
                assert isinstance(product.local_host, app.LocalHostState.RUNNING)
                assert product.local_host.host.backend == host_snapshot.backend
                # The native service dispatcher owns this lane while JS is absent.
                try:
                    borrowed.application_events()
                    raise AssertionError("app-owned event lane was claimable")
                except sdk.BindingError.AlreadyClaimed:
                    pass
                del borrowed
                gc.collect()
                assert (await app.read_snapshot()).local_host.is_running()
            finally:
                assert isinstance(app.native_stop(), app.DevelopmentNodeStopOutcome.STOPPED)
            assert app.shared_host() is None
            assert app.native_inspect_identity(root).identity_hash == identity
        assert isinstance(app.native_reset(root), (
            app.DevelopmentNodeStopOutcome.STOPPED,
            app.DevelopmentNodeStopOutcome.ALREADY_STOPPED,
        ))
        assert isinstance(app.native_inspect_identity(root), app.PrimaryIdentityState.MISSING)
    print("Aggregate foreign ownership, native event claim, retained identity, restart and reset passed")


asyncio.run(run())
