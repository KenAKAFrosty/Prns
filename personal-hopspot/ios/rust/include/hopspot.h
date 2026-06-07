//WIP NEEDS REVIEW
#ifndef HOPSPOT_H
#define HOPSPOT_H

#include <stdint.h>
#include <stddef.h>

typedef struct HopspotFace HopspotFace;

HopspotFace *hopspot_init(void);
void hopspot_free(HopspotFace *handle);
int32_t hopspot_post_input(HopspotFace *handle, int32_t code);
void hopspot_render(HopspotFace *handle, uint8_t *ptr, size_t len);
uint32_t hopspot_panel_width(void);
uint32_t hopspot_panel_height(void);

#endif
