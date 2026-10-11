/* crab_rade.c — crabSDR-Decoder für RADE (FreeDV ab Version 2.0: Radio Autoencoder, OFDM mit neuronalem Sprachcodec).
 *
 * Liest USB-Ton (s16le mono, 8000 Hz) von stdin, demoduliert RADE V1 (Vorgabe) oder V2 mit rade_c (github.com/freedv/rade_c,
 * BSD-2-Clause) und synthetisiert die Sprache mit FARGAN (Opus). Ausgabe: Sprache als s16le mono 16000 Hz auf stdout,
 * crabSDR verteilt sie als hörbaren Kanal. Ereignisse als JSON-Zeilen auf stderr:
 *   {"kind":"sync","mode":"RADE V1","on":true,"snr":12.3}   beim Kommen und Gehen des Signals (entprellt, siehe unten)
 *   {"kind":"eoo","mode":"RADE V1","snr":12.3}              Ende der Durchgabe erkannt (EOO-Rahmen)
 * Alle anderen stderr-Zeilen sind Logtext (Start, Fehler).
 *
 * Das reelle SSB-Signal geht ohne Hilbert-Transformation als I (Q = 0) in den Demodulator: die OFDM-Träger liegen bei
 * 1062–1875 Hz, das Spiegelbild bei negativen Frequenzen weisen die Korrelatoren ab (so macht es auch rade_rx_wav).
 * Skalierung int16 → float mit 2/RADE_INT16_SCALE wie in rade_api.h beschrieben; die Erfassung passt ihre Schwelle an
 * den Rauschpegel an, der absolute Pegel ist unkritisch (getestet −20 … +6 dB um den Nennpegel).
 *
 * Optionen (Umgebung, von crabSDR aus `options` in config.toml als CRAB_OPT_<NAME>):
 *   SNR_MIN  Sync zählt erst ab so viel dB (Standard aus = -100: die Sync-Zustandsmaschine von RADE lockt auf Rauschen nicht,
 *            und schwache Gegenstationen liegen bei -2 dB noch lesbar; die Schätzung bei gutem Signal 3 … 20 dB)
 *   HOLD     Sekunden Tonzeit, die der Sync stehen muss, bevor er gemeldet wird, und die er nach dem Verlust noch gilt (1.0)
 *   GAIN     Faktor auf die Eingangswerte (1.0)
 *   V2       1 = RADE V2 statt V1 (V2 ist noch in Entwicklung, Wellenform nicht endgültig)
 *   VERBOSE  1 = Statuszeilen des Demodulators auf stderr (nur zum Testen)
 * Aufruf zum Testen ohne crabSDR:  sox aufnahme.wav -r 8000 -t s16 -c 1 - | ./crab_rade | sox -t s16 -r 16000 -c 1 - sprache.wav
 */
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <stdint.h>
#include <math.h>

#include "rade_api.h"
#include "rade_dsp.h"
#include "fargan.h"
#include "lpcnet.h"

static double env_num(const char *name, double def) {
    const char *v = getenv(name);
    if (!v || !*v) return def;
    char *end; double d = strtod(v, &end);
    return (end == v) ? def : d;
}

static void say_sync(const char *mode, int on, float snr) {
    fprintf(stderr, "{\"kind\":\"sync\",\"mode\":\"%s\",\"on\":%s,\"snr\":%.1f}\n", mode, on ? "true" : "false", (double)snr);
    fflush(stderr);
}

int main(int argc, char *argv[]) {
    int   v2      = (int)env_num("CRAB_OPT_V2", 0);
    int   verbose = (int)env_num("CRAB_OPT_VERBOSE", 0);
    float snr_min = (float)env_num("CRAB_OPT_SNR_MIN", -100.0);
    float hold    = (float)env_num("CRAB_OPT_HOLD", 1.0);
    float gain    = (float)env_num("CRAB_OPT_GAIN", 1.0);
    for (int i = 1; i < argc; i++) {
        if (!strcmp(argv[i], "--v2")) v2 = 1;
        else if (!strcmp(argv[i], "-v") && i + 1 < argc) verbose = atoi(argv[++i]);
        else if (!strcmp(argv[i], "-h") || !strcmp(argv[i], "--help")) {
            fprintf(stderr, "crab_rade [--v2] [-v N]: s16le 8 kHz USB-Ton auf stdin → RADE-Sprache s16le 16 kHz auf stdout, JSON-Ereignisse auf stderr\n");
            return 0;
        }
    }
    int rate = (int)env_num("CRAB_RATE", RADE_FS);
    if (rate != RADE_FS) fprintf(stderr, "crab_rade: Modem will %d Hz, crabSDR liefert %d – rate im Plugin anpassen\n", RADE_FS, rate);
    const char *mode = v2 ? "RADE V2" : "RADE V1";

    rade_initialize();
    int flags = 0;
    if (verbose == 0) flags |= RADE_VERBOSE_0;
    else if (verbose == 1) flags |= RADE_VERBOSE_TERSE;
    else flags |= RADE_VERBOSE_FULL;
    if (v2) flags |= RADE_MODE_V2;
    struct rade *r = rade_open("(eingebaute Gewichte)", flags);
    if (!r) { fprintf(stderr, "crab_rade: rade_open fehlgeschlagen\n"); return 1; }

    int nin_max        = rade_nin_max(r);
    int n_features_out = rade_n_features_in_out(r);
    int n_eoo_bits     = rade_n_eoo_bits(r);
    int16_t   *pcm_in  = malloc((size_t)nin_max * sizeof(int16_t));
    RADE_COMP *rx_buf  = malloc((size_t)nin_max * sizeof(RADE_COMP));
    float     *feat    = malloc((size_t)n_features_out * sizeof(float));
    float     *eoo     = n_eoo_bits ? malloc((size_t)n_eoo_bits * sizeof(float)) : NULL;
    if (!pcm_in || !rx_buf || !feat || (n_eoo_bits && !eoo)) { fprintf(stderr, "crab_rade: kein Speicher\n"); return 1; }

    FARGANState fargan; fargan_init(&fargan);
    float cont_buf[5 * NB_TOTAL_FEATURES]; int cont_frames = 0, fargan_ready = 0;

    fprintf(stderr, "crab_rade: %s, rade_c API %d.%d, nin_max %d, snr_min %.1f dB, hold %.1f s\n", mode, rade_version(), rade_version_minor(), nin_max, (double)snr_min, (double)hold);

    const float scale = (2.0f / RADE_INT16_SCALE) * gain;
    double t_audio = 0.0, since = -1.0, lost = -1.0;   /* Entprellung nach Tonzeit, nicht Wanduhr */
    int on = 0; float snr = 0.0f;
    long frames_out = 0;

    for (;;) {
        int nin = rade_nin(r);
        size_t got = fread(pcm_in, sizeof(int16_t), (size_t)nin, stdin);
        if (got < (size_t)nin) {
            if (got == 0) break;
            memset(pcm_in + got, 0, ((size_t)nin - got) * sizeof(int16_t));   /* letzter Block: mit Stille auffüllen */
        }
        for (int i = 0; i < nin; i++) { rx_buf[i].real = pcm_in[i] * scale; rx_buf[i].imag = 0.0f; }

        int has_eoo = 0;
        int n_out = rade_rx(r, feat, &has_eoo, eoo, rx_buf);
        int sync = rade_sync(r);
        snr = rade_snrdB_3k_est(r);
        t_audio += (double)nin / RADE_FS;

        int good = sync && snr >= snr_min;
        if (good) {
            if (since < 0) since = t_audio;
            lost = -1.0;
            if (!on && t_audio - since >= hold) { on = 1; say_sync(mode, 1, snr); }
        } else {
            since = -1.0;
            if (on) {
                if (lost < 0) lost = t_audio;
                else if (t_audio - lost >= hold) { on = 0; say_sync(mode, 0, snr); }
            }
        }
        if (has_eoo) {
            fprintf(stderr, "{\"kind\":\"eoo\",\"mode\":\"%s\",\"snr\":%.1f}\n", mode, (double)snr); fflush(stderr);
        }

        if (n_out > 0 && (on || good)) {
            int n_frames = n_out / RADE_NB_TOTAL_FEATURES;
            for (int fi = 0; fi < n_frames; fi++) {
                float *f = &feat[fi * RADE_NB_TOTAL_FEATURES];
                if (!fargan_ready) {
                    /* fargan_cont braucht 5 Rahmen Vorlauf (wie lpcnet_demo / rade_rx_wav) */
                    memcpy(&cont_buf[cont_frames * NB_TOTAL_FEATURES], f, NB_TOTAL_FEATURES * sizeof(float));
                    if (++cont_frames >= 5) {
                        float packed[5 * NB_FEATURES];
                        for (int i = 0; i < 5; i++) memcpy(&packed[i * NB_FEATURES], &cont_buf[i * NB_TOTAL_FEATURES], NB_FEATURES * sizeof(float));
                        float zeros[FARGAN_CONT_SAMPLES]; memset(zeros, 0, sizeof(zeros));
                        fargan_cont(&fargan, zeros, packed);
                        fargan_ready = 1;
                    }
                    continue;
                }
                float fpcm[LPCNET_FRAME_SIZE]; int16_t pcm[LPCNET_FRAME_SIZE];
                fargan_synthesize(&fargan, fpcm, f);
                for (int s = 0; s < LPCNET_FRAME_SIZE; s++) {
                    float v = fpcm[s] * 32768.0f;
                    if (v > 32767.0f) v = 32767.0f; if (v < -32767.0f) v = -32767.0f;
                    pcm[s] = (int16_t)floor(0.5 + (double)v);
                }
                if (fwrite(pcm, sizeof(int16_t), LPCNET_FRAME_SIZE, stdout) != LPCNET_FRAME_SIZE) goto done;
                frames_out++;
            }
            fflush(stdout);
        }
        if (got < (size_t)nin) break;
    }
done:
    if (on) say_sync(mode, 0, snr);
    fprintf(stderr, "crab_rade: Ende nach %.1f s Ton, %.1f s Sprache\n", t_audio, frames_out * (double)LPCNET_FRAME_SIZE / RADE_FS_SPEECH);
    free(pcm_in); free(rx_buf); free(feat); free(eoo);
    rade_close(r); rade_finalize();
    return 0;
}
