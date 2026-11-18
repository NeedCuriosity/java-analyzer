package com.example.modern;

import java.util.List;
import java.util.function.BiFunction;
import java.util.function.Function;
import java.util.function.Supplier;

public sealed interface Modern permits Modern.Circle, Modern.Square, Modern.Empty {

    record Circle(double radius) implements Modern {
        public Circle {
            if (radius < 0) throw new IllegalArgumentException("negative");
        }

        static Circle unit() {
            return new Circle(1);
        }
    }

    record Square(double side, @Deprecated String label) implements Modern {
        Square(double side) {
            this(side, "square");
        }
    }

    record Pair<A, B>(A first, B second) {}

    final class Empty implements Modern {}

    non-sealed interface Open extends Runnable {}

    static double area(Modern shape) {
        return switch (shape) {
            case Circle c -> Math.PI * c.radius() * c.radius();
            case Square(double side, String label) when side > 10 -> {
                double result = side * side;
                yield result;
            }
            case Square s -> s.side() * s.side();
            case Empty e -> 0;
        };
    }

    static String describe(Object o) {
        if (o instanceof Pair<?, ?>(Circle(var r), var second) && r > 0) {
            return "pair of circle " + r + second;
        }
        if (!(o instanceof String s)) {
            return "other";
        }
        return s.trim();
    }

    static int oldSwitch(int day) {
        int result = 0;
        switch (day) {
            case 1:
            case 2:
                result = 1;
                break;
            case 3, 4 -> result = 2;
            default:
                result = -1;
        }
        return result;
    }

    static String nullSwitch(Object o) {
        return switch (o) {
            case null -> "null";
            case Integer i when i > 0 -> "positive";
            case null, default -> "other";
        };
    }

    static void lambdas() {
        Runnable r = () -> {};
        Function<String, Integer> length = s -> s.length();
        Function<String, Integer> ref = String::length;
        BiFunction<Integer, Integer, Integer> add = (a, b) -> a + b;
        BiFunction<Integer, Integer, Integer> typed = (Integer a, Integer b) -> {
            return a * b;
        };
        BiFunction<Integer, Integer, Integer> inferred = (var a, var b) -> a - b;
        Supplier<List<String>> ctor = java.util.ArrayList::new;
        Function<Integer, int[]> arrays = int[]::new;
        Supplier<Supplier<String>> nested = () -> () -> "x";
        Object cast = (Runnable & java.io.Serializable) () -> {};
        int generic = Modern.<Integer>identity(1);
        var unnamed = add.andThen(_ -> 0);
    }

    static <T> T identity(T value) {
        return value;
    }

    static String blocks() {
        String json = """
            {
              "name": "value \""" inside",
              "n": 1
            }
            """;
        var yield = 1;
        var record = 2;
        var sealed = 3;
        var permits = yield + record + sealed;
        return json + permits;
    }

    static void generics() {
        List<? super Integer> lower = new java.util.ArrayList<Number>();
        java.util.Map<String, List<java.util.Map.Entry<String, int[]>>> deep = new java.util.HashMap<>();
        boolean cmp = lower.size() < 3 && deep.size() > (2);
        int shift = lower.size() >> 1;
        List<List<List<String>>> triple = null;
        @SuppressWarnings("unused") final int annotated = 1;
        java.util.List<@Deprecated String> typeUse;
    }
}

class CastInCaseLabel {
    static final long A = 1, B = 2;

    int f(long x) {
        return switch ((int) x) {
            case (int) A, (int) B -> 16;
            default -> 0;
        };
    }
}

class ArrayTypeAnnotations {
    @interface N {}

    transient int @N [] entries;
    transient @N Object @N [] @N [] keys;

    Object @N [] f(String @N ... rest) {
        int @N [] local = entries;
        return keys[0];
    }
}

class WildcardTypeAnnotations {
    @interface N {}

    java.util.List<@N ? extends @N Number> numbers;

    void f(java.util.List<java.util.List<@N ?>> lists) {
        for (java.util.List<@N ?> list : lists) {
            java.util.Map<@N ? super String, @N ?> map = null;
        }
    }
}
