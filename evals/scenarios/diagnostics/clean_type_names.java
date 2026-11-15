//- /app/Shapes.java
package app;

import java.util.List;
import java.util.Map;
import java.util.Set;
import java.util.stream.Collector;

public class Shapes {
    public interface Kinded {
        enum Kind { ROUND, FLAT }
        Kind kind();
    }

    // 记录头部用到了从接口继承来的成员类型
    public record Circle(Kind kind, List<Kind> others) implements Kinded {}

    record Impl<T>(Set<Characteristics> characteristics) implements Collector<T, T, T> {
        public java.util.function.Supplier<T> supplier() { return null; }
        public java.util.function.BiConsumer<T, T> accumulator() { return null; }
        public java.util.function.BinaryOperator<T> combiner() { return null; }
        public java.util.function.Function<T, T> finisher() { return null; }
    }

    public class Inner {
        class Deep {}
    }

    static class Sub extends Shapes {
        Inner inherited;
        Map.Entry<String, Inner> entry;
    }

    <T extends Comparable<T>, U extends List<? super T>> U generic(T t, U u) { return u; }

    Object run(Shapes other, Sub sub) {
        // 限定的 new：类型名是限定表达式类型的成员
        Inner a = other.new Inner();
        Inner.Deep b = a.new Deep();
        Object c = sub.new Inner() {};
        java.util.concurrent.atomic.AtomicInteger d = new java.util.concurrent.atomic.AtomicInteger();
        class Local<E> {
            E value;
            Local<E> self() { return this; }
        }
        Local<String> local = new Local<>();
        var inferred = local.self();
        int[] numbers = new int[0];
        String[][] grid = new String[1][1];
        Runnable r = () -> { Thread t = Thread.currentThread(); t.getName(); };
        return List.of(a, b, c, d, inferred, numbers, grid, r);
    }
}
//? diag none
