/*
 * Header comment.
 */
package com.example.basics;

import java.util.ArrayList;
import java.util.List;
import java.util.Map;
import static java.util.Objects.requireNonNull;
import static java.lang.Math.*;

/**
 * Basic declarations.
 *
 * @param <T> element type
 */
@SuppressWarnings({"unchecked", "rawtypes"})
public abstract class Basics<T extends Comparable<? super T>> extends Object implements Runnable, Cloneable {
    public static final int LIMIT = 0x7fff_ffff;
    private static final long BIG = 1_000_000L, SMALL = 07L;
    protected volatile transient double ratio = 1.5e-3d;
    float f = .5f;
    char c = 'A', quote = '\'';
    String text = "a \"quoted\" \\ string\n";
    int[] numbers = {1, 2, 3,};
    int[][] grid = new int[3][];
    List<Map<String, ? extends Number>>[] generics;
    byte b = (byte) 0b1010;

    static {
        System.out.println("static init");
    }

    {
        ratio = 2;
    }

    protected Basics() {
        this(null);
    }

    Basics(T seed) throws IllegalStateException {
        super();
        requireNonNull(seed);
    }

    public abstract T next();

    @Override
    public void run() {
        int i = 0, j;
        final var list = new ArrayList<String>();
        for (int k = 0, n = 10; k < n; k++, i += 2) {
            if (k % 2 == 0) continue;
            else if (k > 7) break;
            list.add(String.valueOf(k));
        }
        for (String s : list) {
            System.out.println(s);
        }
        outer:
        while (true) {
            do {
                i--;
                if (i < 0) break outer;
            } while (i > 0);
        }
        j = i > 0 ? i : -i;
        i <<= 2;
        i >>= 1;
        i >>>= 1;
        j = i >> 1 >>> 2 << 3;
        boolean flag = !(i < j) && j >= 0 || i != j ^ true;
        long big = (long) i * j + ~i - +j;
        Object o = (Object) list;
        if (o instanceof List<?> && flag) {
            assert big > 0 : "must be positive";
        }
        synchronized (this) {
            numbers[0]++;
            ++numbers[1];
        }
        ;
    }

    public <U, V extends U> V convert(U input, Class<V> type, int... rest) {
        return type.cast(input);
    }

    int legacy()[] {
        return numbers;
    }

    static int tryIt(String path) {
        try (java.io.Reader reader = new java.io.StringReader(path); var other = reader) {
            return reader.read();
        } catch (java.io.IOException | RuntimeException e) {
            throw new IllegalStateException(e);
        } finally {
            System.out.println("done");
        }
    }

    interface Visitor<R> {
        R visit(Basics<?> node);

        default R visitAll(List<? extends Basics<?>> nodes) {
            R last = null;
            for (Basics<?> node : nodes) last = visit(node);
            return last;
        }

        static <R> Visitor<R> constant(R value) {
            return node -> value;
        }
    }

    enum Color implements Runnable {
        RED(1), GREEN(2) {
            @Override
            public void run() {
                System.out.println("green");
            }
        },
        BLUE(3);

        private final int code;

        Color(int code) {
            this.code = code;
        }

        public void run() {}
    }

    @interface Marker {
        String value() default "";
        int[] counts() default {1, 2};
        Class<?> type() default Object.class;
    }

    class Inner {
        Object outer() {
            return Basics.this;
        }
    }

    Object anonymous() {
        return new Basics<T>(null) {
            @Override
            public T next() {
                return null;
            }
        };
    }

    Inner qualifiedNew(Basics<T> other) {
        return other.new Inner();
    }
}
