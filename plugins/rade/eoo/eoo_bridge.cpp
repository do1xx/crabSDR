/* eoo_bridge.cpp — C-Brücke zu EooCallsignDecoder (siehe eoo_bridge.h). Teil von crabSDR (AGPL 3.0 oder neuer). */
#include "eoo_bridge.h"
#include "EooCallsignCodec.h"
#include <cstdio>
#include <string>

extern "C" int eoo_callsign_decode(const float *syms, int n_syms, char *out, int out_len) {
    if (!syms || n_syms < 56 || !out || out_len < 2) return 0;
    std::string cs;
    EooCallsignDecoder dec;
    if (!dec.decode(syms, n_syms, cs) || cs.empty()) return 0;
    std::snprintf(out, (size_t)out_len, "%s", cs.c_str());
    return 1;
}

extern "C" void eoo_callsign_encode(const char *callsign, float *syms, int n_floats) {
    EooCallsignDecoder enc;
    enc.encode(std::string(callsign ? callsign : ""), syms, n_floats);
}
