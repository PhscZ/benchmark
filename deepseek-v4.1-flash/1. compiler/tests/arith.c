int printf(char *fmt, ...);

int calls;

int side(int v) {
    calls = calls + 1;
    return v;
}

int main(void) {
    int a = 17;
    int b = 5;

    printf("%d %d %d %d %d\n", a + b, a - b, a * b, a / b, a % b);
    printf("%d %d %d\n", a & b, a | b, a ^ b);
    printf("%d %d\n", a << 2, a >> 1);
    printf("%d %d %d %d %d %d\n", a < b, a <= b, a > b, a >= b, a == b, a != b);
    printf("%d\n", 1 + 2 * 3 - 4 / 2);
    printf("%d\n", (1 + 2) * 3);
    printf("%d\n", 1 << 3 | 1);
    printf("%d\n", 7 & 3 ^ 1);
    printf("%d\n", -7 / 2);
    printf("%d\n", -7 % 2);
    printf("%d\n", 7 / -2);
    printf("%d\n", ~0);
    printf("%d %d\n", !0, !5);
    printf("%d %d %d\n", 2 + 3 * 4 - 5, 100 / 10 / 2, 1 - 2 - 3);
    printf("%d %d\n", 10 > 5 == 1, 1 < 2 < 3);
    printf("%d\n", 1 + 1 == 2 && 3 * 3 == 9);
    printf("%d\n", 2 - 3 - 4 * -5);
    printf("%d %d\n", 40 >> 2, 40 << 1);
    printf("%d\n", 1 | 2 | 4 | 8);
    printf("%d\n", (5 ^ 3) & 6);

    // short-circuit: the right operand must not be evaluated
    calls = 0;
    printf("%d %d\n", 0 && side(1), calls);
    calls = 0;
    printf("%d %d\n", 1 || side(1), calls);
    calls = 0;
    printf("%d %d\n", 1 && side(1), calls);
    calls = 0;
    printf("%d %d\n", 0 || side(1), calls);

    // ternary and comma
    printf("%d %d\n", a > b ? a : b, a < b ? a : b);
    printf("%d\n", (a = 3, a + 1));

    // assignment and compound assignment
    a = 10;
    a += 5;
    printf("%d\n", a);
    a -= 3;
    printf("%d\n", a);
    a *= 4;
    printf("%d\n", a);
    a /= 6;
    printf("%d\n", a);
    a %= 4;
    printf("%d\n", a);
    a <<= 3;
    printf("%d\n", a);
    a >>= 2;
    printf("%d\n", a);
    a |= 8;
    printf("%d\n", a);
    a &= 12;
    printf("%d\n", a);
    a ^= 5;
    printf("%d\n", a);

    // increment / decrement (each step is a separate full expression)
    a = 5;
    printf("%d\n", a++);
    printf("%d\n", a);
    printf("%d\n", ++a);
    printf("%d\n", a);
    printf("%d\n", a--);
    printf("%d\n", a);
    printf("%d\n", --a);
    printf("%d\n", a);

    // postfix/prefix on array elements and through pointers
    {
        int arr[3];
        int *q;
        arr[0] = 10;
        arr[1] = 20;
        arr[2] = 30;
        q = arr;
        printf("%d\n", (*q)++);
        printf("%d\n", arr[0]);
        printf("%d\n", *q++);
        printf("%d\n", *q);
        printf("%d\n", ++*q);
        printf("%d\n", arr[1]);
        printf("%d\n", (*q)--);
        printf("%d\n", arr[1]);
        printf("%d\n", q - arr);
        printf("%d\n", q[1]);
    }

    // chained assignment
    a = b = 42;
    printf("%d %d\n", a, b);
    printf("%d\n", a = 3);

    // unary minus and nesting
    printf("%d\n", -(-(-3)));
    printf("%d\n", -a + b);
    printf("%d\n", !!7);
    return 0;
}
