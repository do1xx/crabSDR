/* rade_eoo_tx.c — Testsender für crab_rade: Sprache (s16le mono 16000 Hz) von stdin → RADE-V1-Signal als reeller USB-Ton
 * (s16le mono 8000 Hz) auf stdout, am Ende ein EOO-Rahmen mit dem Rufzeichen aus argv[1] (Format der FreeDV-App, LDPC + CRC,
 * kodiert mit EooCallsignCodec). Nicht Teil des Plugins, nur zum Prüfen; Bauen: EOO_TX=1 plugins/rade/build.sh → build/rade_eoo_tx.
 *   sox sprache.wav -t s16 -r 16000 -c 1 - | ./build/rade_eoo_tx DO1XX | ./crab_rade > /dev/null
 * Ohne Ton (stdin leer) werden 3 s Stille gesendet. Vorher und nachher je 1 s Stille, damit der Empfänger wie on air einrastet. */
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <stdint.h>
#include <math.h>

#include "rade_api.h"
#include "rade_dsp.h"
#include "lpcnet.h"
#include "arch.h"
#include "cpu_support.h"
#include "eoo/eoo_bridge.h"

static void write_real(const RADE_COMP *iq, int n) {
    int16_t buf[4096];
    while (n > 0) {
        int m = n < 4096 ? n : 4096;
        for (int i = 0; i < m; i++) {
            float v = iq[i].real * RADE_INT16_SCALE;
            if (v > 32767.0f) v = 32767.0f; if (v < -32767.0f) v = -32767.0f;
            buf[i] = (int16_t)floor(0.5 + (double)v);
        }
        fwrite(buf, sizeof(int16_t), (size_t)m, stdout);
        iq += m; n -= m;
    }
}

static void write_silence(double sec) {
    int16_t z[800]; memset(z, 0, sizeof(z));
    for (int n = (int)(sec * RADE_FS); n > 0; n -= 800) fwrite(z, sizeof(int16_t), (size_t)(n < 800 ? n : 800), stdout);
}

int main(int argc, char *argv[]) {
    const char *callsign = argc > 1 ? argv[1] : "DO1XX";
    rade_initialize();
    struct rade *r = rade_open("(eingebaute Gewichte)", RADE_VERBOSE_0);
    if (!r) { fprintf(stderr, "rade_eoo_tx: rade_open fehlgeschlagen\n"); return 1; }
    int n_features_in = rade_n_features_in_out(r), n_tx_out = rade_n_tx_out(r), n_eoo_out = rade_n_tx_eoo_out(r);
    int n_eoo_bits = rade_n_eoo_bits(r);
    int frames_per_mf = n_features_in / RADE_NB_TOTAL_FEATURES;
    float *feat = malloc((size_t)n_features_in * sizeof(float));
    RADE_COMP *tx = malloc((size_t)(n_tx_out > n_eoo_out ? n_tx_out : n_eoo_out) * sizeof(RADE_COMP));
    float *eoo_bits = malloc((size_t)n_eoo_bits * sizeof(float));
    if (!feat || !tx || !eoo_bits) { fprintf(stderr, "rade_eoo_tx: kein Speicher\n"); return 1; }

    eoo_callsign_encode(callsign, eoo_bits, n_eoo_bits);
    rade_tx_set_eoo_bits(r, eoo_bits);

    int arch = opus_select_arch();
    LPCNetEncState *net = lpcnet_encoder_create();
    if (!net) { fprintf(stderr, "rade_eoo_tx: lpcnet_encoder_create fehlgeschlagen\n"); return 1; }

    write_silence(1.0);
    int16_t pcm[LPCNET_FRAME_SIZE]; int fi = 0, mf = 0, eof = 0; long frames_in = 0;
    const long min_frames = 3 * RADE_FS_SPEECH / LPCNET_FRAME_SIZE;   /* mindestens 3 s, sonst mit Stille auffüllen */
    for (;;) {
        size_t got = eof ? 0 : fread(pcm, sizeof(int16_t), LPCNET_FRAME_SIZE, stdin);
        if (got < LPCNET_FRAME_SIZE) {
            if (got == 0) { eof = 1; if (frames_in >= min_frames) break; }
            memset(pcm + got, 0, (LPCNET_FRAME_SIZE - got) * sizeof(int16_t));
        }
        frames_in++;
        lpcnet_compute_single_frame_features(net, pcm, &feat[fi * RADE_NB_TOTAL_FEATURES], arch);
        if (++fi >= frames_per_mf) {
            write_real(tx, rade_tx(r, tx, feat)); fi = 0; mf++;
        }
    }
    if (fi > 0) {
        memset(&feat[fi * RADE_NB_TOTAL_FEATURES], 0, (size_t)(frames_per_mf - fi) * RADE_NB_TOTAL_FEATURES * sizeof(float));
        write_real(tx, rade_tx(r, tx, feat)); mf++;
    }
    write_real(tx, rade_tx_eoo(r, tx));
    write_silence(1.0);
    fflush(stdout);
    fprintf(stderr, "rade_eoo_tx: %d Modemrahmen + EOO mit Rufzeichen %s (%.1f s Sprache)\n", mf, callsign, frames_in * (double)LPCNET_FRAME_SIZE / RADE_FS_SPEECH);
    lpcnet_encoder_destroy(net);
    free(feat); free(tx); free(eoo_bits);
    rade_close(r); rade_finalize();
    return 0;
}
