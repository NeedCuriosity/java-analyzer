import java.util.List;
import java.util.concurrent.atomic.AtomicReferenceArray;

class Holder {
    interface Entry<K, V, E extends Entry<K, V, E>> {
        K key();
    }

    static class Segment<K, V, E extends Entry<K, V, E>, S extends Segment<K, V, E, S>> {
        AtomicReferenceArray<E> table;
        E first;
        List<E> entries;

        E head() {
            return first;
        }

        AtomicReferenceArray<E> copy() {
            return table;
        }
    }

    static class Box<T extends Number> {
        T value;
        List<T> values;

        T get() {
            return value;
        }
    }

    void captures(Segment<Object, Object, ?, ?> segment, Box<?> box, Box<? super Integer> lower) {
        // 捕获变量的上界是通配符上界与声明上界的交
        AtomicReferenceArray<? extends Entry<Object, Object, ?>> table = segment.table;
        AtomicReferenceArray<? extends Entry<Object, Object, ?>> copy = segment.copy();
        Entry<Object, Object, ?> first = segment.first;
        Entry<Object, Object, ?> head = segment.head();
        List<? extends Entry<Object, Object, ?>> entries = segment.entries;
        Number value = box.value;
        Number got = box.get();
        List<? extends Number> values = box.values;
        Number lowerValue = lower.get();
        System.out.println("" + table + copy + first + head + entries + value + got + values + lowerValue);
    }
}

class Red<A> {
    class Orange {
        Red<A> getSelfA() {
            return Red.this;
        }

        List<A> items() {
            return null;
        }

        void accept(Red<A> red) {}

        Red<A> self() {
            // 在内部类自身里，外层类的类型参数就是作用域里的那个
            return getSelfA();
        }
    }

    // 继承来的成员要用 `Red<B>.Orange` 的外层类型实参代换
    class Yellow<B> extends Red<B>.Orange {
        Yellow(Red<B> red) {
            red.super();
        }

        Red<B> getB() {
            return getSelfA();
        }

        List<B> itemsB() {
            return items();
        }

        void pass(Red<B> red) {
            accept(red);
        }
    }
}
//? diag none
