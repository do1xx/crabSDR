/* eoo_bridge.h — C-Schnittstelle zu EooCallsignCodec (C++), damit crab_rade.c das Rufzeichen aus dem EOO-Rahmen lesen
 * und der Testsender eines erzeugen kann. Das Modul EooCallsignCodec.{h,cpp} stammt aus github.com/peterbmarks/radae_decoder
 * (LGPL 2.1, herausgelöst aus codec2 und FreeDV-GUI), siehe THIRD-PARTY.md. */
#ifndef EOO_BRIDGE_H
#define EOO_BRIDGE_H
#ifdef __cplusplus
extern "C" {
#endif
/* syms: die rade_n_eoo_bits() Soft-Werte aus rade_rx() (I/Q-Paare), n_syms = rade_n_eoo_bits()/2.
 * Liefert 1 und das Rufzeichen (NUL-terminiert, höchstens out_len-1 Zeichen) bei LDPC-BER < 0,2 und passender CRC-8, sonst 0. */
int  eoo_callsign_decode(const float *syms, int n_syms, char *out, int out_len);
/* Rufzeichen (bis 8 Zeichen A–Z 0–9 &'()*+,-./) in rade_n_eoo_bits() = n_floats Werte für rade_tx_set_eoo_bits() kodieren. */
void eoo_callsign_encode(const char *callsign, float *syms, int n_floats);
#ifdef __cplusplus
}
#endif
#endif
