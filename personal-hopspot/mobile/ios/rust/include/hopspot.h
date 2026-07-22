#ifndef HOPSPOT_H
#define HOPSPOT_H

#include <stdint.h>
#include <stddef.h>
#include <stdbool.h>

typedef struct HopspotFace HopspotFace;

void hopspot_start_engine(void);
HopspotFace *hopspot_init(void);
void hopspot_free(HopspotFace *handle);
int32_t hopspot_post_input(HopspotFace *handle, int32_t code);
void hopspot_announce(void);
void hopspot_render(HopspotFace *handle, uint8_t *ptr, size_t len);
void hopspot_set_battery(HopspotFace *handle, int32_t percent, bool charging);
uint32_t hopspot_panel_width(void);
uint32_t hopspot_panel_height(void);

#endif
