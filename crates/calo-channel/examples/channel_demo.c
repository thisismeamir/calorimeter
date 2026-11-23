#include "calo_channel.h"

int main(void) {
    CALO_PHASE_BEGIN("init");
    calo_tag("integrator", "verlet");
    CALO_PHASE_END("init");

    CALO_PHASE_BEGIN("main");
    CALO_REGION_BEGIN("field_update");
    CALO_REGION_BEGIN("fft");
    CALO_REGION_END("fft");
    CALO_REGION_END("field_update");
    calo_metric("steps", 100.0, "steps");
    CALO_PHASE_END("main");
    return 0;
}
