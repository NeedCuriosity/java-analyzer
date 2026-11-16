//- /app/Counter.java before
package app;

import java.util.function.IntBinaryOperator;

class Counter {
    private int next = 0;

    int add(int a, int b) {
        return a + b;
    }

    int run() {
        IntBinaryOperator op = this::add;
        return add(1, next++) + op.applyAsInt(1, 2);
    }
}
//- /app/Counter.java
package app;

import java.util.function.IntBinaryOperator;

class Counter {
    private int next = 0;

    int /*@decl*/add(int a) {
        return a + b;
    }

    int /*@other*/run() {
        IntBinaryOperator op = this::add;
        return add(1, next++) + op.applyAsInt(1, 2);
    }
}
//? assist change_signature decl ; fails
//? noassist change_signature other
