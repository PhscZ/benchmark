/* Linked into the GCC reference builds only.
 *
 * The programs produced by ccomp switch stdin/stdout to binary mode at the
 * start of main.  Without the same setup the GCC build would translate CRLF on
 * output and treat 0x1A as end-of-file on input, so its byte stream could not
 * be compared against the ccomp build.
 */
int _setmode(int fd, int mode);

__attribute__((constructor)) static void ref_binary_streams(void) {
    _setmode(0, 0x8000);
    _setmode(1, 0x8000);
}
