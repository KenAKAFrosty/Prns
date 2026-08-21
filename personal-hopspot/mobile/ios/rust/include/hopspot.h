#ifndef HOPSPOT_H
#define HOPSPOT_H

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

typedef struct HopspotFace HopspotFace;

typedef enum HopspotInputCode {
    HopspotInputShortPress = 0,
    HopspotInputLongPress = 1,
} HopspotInputCode;

typedef enum HopspotActionCode {
    HopspotActionNone = 0,
    HopspotActionAnnounce = 1,
} HopspotActionCode;

typedef enum HopspotEngineState {
    HopspotEngineStopped = 0,
    HopspotEngineStarting = 1,
    HopspotEngineRunning = 2,
    HopspotEngineFailed = 3,
} HopspotEngineState;

typedef enum HopspotEngineFailure {
    HopspotEngineFailureNone = 0,
    HopspotEngineFailureStorageConfiguration = 1,
    HopspotEngineFailureWorkerSpawn = 2,
    HopspotEngineFailureRuntimeBuild = 3,
    HopspotEngineFailureLocalListenerBind = 4,
    HopspotEngineFailureRpcListenerBind = 5,
    HopspotEngineFailureStartupTimeout = 6,
    HopspotEngineFailureWorkerStopped = 7,
    HopspotEngineFailureShutdownTimeout = 8,
    HopspotEngineFailurePersistenceWrite = 9,
} HopspotEngineFailure;

int32_t hopspot_start_engine(const char *storage_directory_utf8);
int32_t hopspot_stop_engine(void);
int32_t hopspot_engine_state(void);
int32_t hopspot_engine_last_failure(void);
HopspotFace *hopspot_init(void);
void hopspot_free(HopspotFace *handle);
int32_t hopspot_post_input(HopspotFace *handle, int32_t code);
void hopspot_announce(void);
void hopspot_render(HopspotFace *handle, uint8_t *ptr, size_t len);
void hopspot_set_battery(HopspotFace *handle, int32_t percent, bool externally_powered);
uint32_t hopspot_panel_width(void);
uint32_t hopspot_panel_height(void);
size_t hopspot_rgba_bytes(void);
uint32_t hopspot_render_interval_millis(void);

#endif
