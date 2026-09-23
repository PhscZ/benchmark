int printf(char *fmt, ...);

int fib(int n);
int add6(int a, int b, int c, int d, int e, int f);
int add10(int a, int b, int c, int d, int e, int f, int g, int h, int i, int j);
void nothing(void);
int ack(int m, int n);
int fact(int n);
int gcd(int a, int b);
int is_even(int n);
int is_odd(int n);

int fib(int n) {
    if (n < 2) {
        return n;
    }
    return fib(n - 1) + fib(n - 2);
}

int add6(int a, int b, int c, int d, int e, int f) {
    return a + b * 2 + c * 3 + d * 4 + e * 5 + f * 6;
}

int add10(int a, int b, int c, int d, int e, int f, int g, int h, int i, int j) {
    return a + b + c + d + e + f + g + h + i + j;
}

void nothing(void) {
    return;
}

int ack(int m, int n) {
    if (m == 0) {
        return n + 1;
    }
    if (n == 0) {
        return ack(m - 1, 1);
    }
    return ack(m - 1, ack(m, n - 1));
}

int fact(int n) {
    if (n <= 1) {
        return 1;
    }
    return n * fact(n - 1);
}

int gcd(int a, int b) {
    while (b != 0) {
        int t = b;
        b = a % b;
        a = t;
    }
    return a;
}

int is_even(int n) {
    if (n == 0) {
        return 1;
    }
    return is_odd(n - 1);
}

int is_odd(int n) {
    if (n == 0) {
        return 0;
    }
    return is_even(n - 1);
}

int main(void) {
    printf("%d\n", fib(20));
    printf("%d\n", fib(0));
    printf("%d\n", fib(1));
    printf("%d\n", add6(1, 2, 3, 4, 5, 6));
    printf("%d\n", add10(1, 2, 3, 4, 5, 6, 7, 8, 9, 10));
    printf("%d\n", ack(2, 3));
    printf("%d\n", fact(10));
    printf("%d\n", gcd(1071, 462));
    printf("%d %d\n", is_even(10), is_odd(10));
    nothing();
    printf("%d\n", add6(fib(5), fib(6), 1, 1, 1, 1));
    return 0;
}
