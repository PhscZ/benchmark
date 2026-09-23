/* static storage: block-scope statics persist across calls, file-scope statics
 * have internal linkage, and both keep their initializers.
 */
int printf(char *fmt, ...);

static int file_static = 100;

static int helper(int x) {
    return x * 2;
}

int counter(void) {
    static int n = 0;
    n = n + 1;
    return n;
}

int accumulate(int v) {
    static int total;
    static int calls;
    calls = calls + 1;
    total = total + v;
    printf("calls=%d total=%d\n", calls, total);
    return total;
}

int memo_fib(int n) {
    static int memo[32];
    if (n < 2) {
        return n;
    }
    if (memo[n] != 0) {
        return memo[n];
    }
    memo[n] = memo_fib(n - 1) + memo_fib(n - 2);
    return memo[n];
}

int bump(void) {
    static char c = 'a';
    static unsigned char u = 250;
    c = c + 1;
    u = u + 1;
    printf("%c %d\n", c, u);
    return 0;
}

int main(void) {
    int i;

    printf("%d %d %d\n", counter(), counter(), counter());
    printf("%d\n", file_static);
    printf("%d\n", helper(21));

    accumulate(1);
    accumulate(2);
    accumulate(3);

    printf("%d %d %d\n", memo_fib(10), memo_fib(20), memo_fib(25));

    for (i = 0; i < 3; i++) {
        bump();
    }

    /* a static shadowed by a later local of the same name */
    {
        static int shadow = 5;
        int shadow2 = shadow;
        printf("%d\n", shadow2);
        {
            int shadow2 = 9;
            printf("%d\n", shadow2);
        }
        printf("%d\n", shadow2);
    }

    /* a local with the same name as a file-scope static */
    {
        int file_static = 1;
        printf("%d\n", file_static);
    }
    printf("%d\n", file_static);

    /* statics inside a loop body keep their value between iterations */
    for (i = 0; i < 4; i++) {
        static int keep = 0;
        keep = keep + 10;
        printf("keep=%d\n", keep);
    }
    return 0;
}
