int printf(char *fmt, ...);

int g;

int main(void) {
    int i;
    int j;
    int sum;

    sum = 0;
    for (i = 0; i < 10; i++) {
        sum += i;
    }
    printf("%d\n", sum);

    i = 0;
    sum = 0;
    while (i < 5) {
        sum += i;
        i++;
    }
    printf("%d\n", sum);

    i = 0;
    sum = 0;
    do {
        sum += i;
        i++;
    } while (i < 5);
    printf("%d\n", sum);

    sum = 0;
    for (i = 0; i < 10; i++) {
        if (i == 3) {
            continue;
        }
        if (i == 7) {
            break;
        }
        sum += i;
    }
    printf("%d\n", sum);

    for (i = 0; i < 3; i++) {
        for (j = 0; j < 3; j++) {
            if (j == 1) {
                continue;
            }
            printf("%d%d ", i, j);
        }
    }
    printf("\n");

    i = 0;
    while (1) {
        i++;
        if (i > 4) {
            break;
        }
    }
    printf("%d\n", i);

    // do/while executes the body at least once
    i = 100;
    sum = 0;
    do {
        sum += 1;
    } while (i < 10);
    printf("%d\n", sum);

    // empty for header, comma in the step clause
    sum = 0;
    for (i = 0, j = 10; i < j; i++, j--) {
        sum++;
    }
    printf("%d\n", sum);

    // if / else if / else chains
    for (i = 0; i < 5; i++) {
        if (i == 0) {
            printf("zero ");
        } else if (i == 1) {
            printf("one ");
        } else if (i == 2) {
            printf("two ");
        } else {
            printf("many ");
        }
    }
    printf("\n");

    // dangling else binds to the nearest if
    i = 1;
    if (i == 1) {
        if (i == 2) {
            printf("inner\n");
        } else {
            printf("else-inner\n");
        }
    }

    // declarations in a nested block, with shadowing
    sum = 0;
    {
        int sum2 = 5;
        {
            int sum2 = 7;
            sum += sum2;
        }
        sum += sum2;
    }
    printf("%d\n", sum);

    // scope of a for-loop declaration
    sum = 0;
    for (int k = 0; k < 4; k++) {
        sum += k;
    }
    printf("%d\n", sum);

    // global mutation
    g = 3;
    g = g * g;
    printf("%d\n", g);

    // loop with no condition body value usage
    sum = 0;
    for (i = 0; ; i++) {
        if (i == 6) {
            break;
        }
        sum += 1;
    }
    printf("%d\n", sum);
    return 0;
}
