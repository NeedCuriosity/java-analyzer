import java.util.AbstractList;
import java.util.ArrayList;
import java.util.Comparator;
import java.util.HashMap;
import java.util.List;
import java.util.Map;
import java.util.function.IntSupplier;
import java.util.function.Supplier;

public class LocalClasses {
    private int base = 10;

    static class Shape {
        static class Point {
            int x;
        }

        int area() {
            return 0;
        }
    }

    int counter(int start) {
        var counter = new Object() {
            int count = start;

            void increment() {
                count++;
            }

            int get() {
                return count;
            }
        };
        counter.increment();
        counter.increment();
        IntSupplier supplier = () -> counter.get();
        return counter.count + supplier.getAsInt();
    }

    int directMember() {
        return new Object() {
            int twice(int x) {
                return x * 2;
            }
        }.twice(21);
    }

    int localClass(int step) {
        int count = 100;
        class Accumulator {
            int count;

            int add() {
                count += step;
                return bump();
            }

            int bump() {
                return count + base;
            }
        }
        Accumulator acc = new Accumulator();
        acc.add();
        int total = new Accumulator().add() + acc.count;
        Supplier<Integer> later = () -> {
            class Nested {
                int value() {
                    return new Accumulator().bump();
                }
            }
            return new Nested().value() + acc.add();
        };
        return total + later.get() + count;
    }

    <T extends Comparable<T>> T generic(T first, T second) {
        class Pair<U> {
            final U left;
            final T right;

            Pair(U left, T right) {
                this.left = left;
                this.right = right;
            }

            <V> V pick(V value) {
                V copy = value;
                return copy;
            }

            T larger(T other) {
                return right.compareTo(other) > 0 ? right : other;
            }
        }
        Pair<String> pair = new Pair<>("x", first);
        String left = pair.left;
        Integer picked = pair.pick(3);
        return pair.larger(second) == null ? first : pair.larger(second);
    }

    int kinds() {
        record Range(int low, int high) {
            int width() {
                return high - low;
            }
        }
        enum Mode {
            FAST, SLOW;

            int weight() {
                return ordinal() + 1;
            }
        }
        interface Scorer {
            int score(int value);

            default int twice(int value) {
                return score(value) * 2;
            }
        }
        Range range = new Range(1, 5);
        Scorer scorer = value -> value + range.width();
        return range.width() + Mode.SLOW.weight() + scorer.twice(range.low());
    }

    int inheritedTypes() {
        class Square extends Shape {
            Point corner = new Point();

            @Override
            int area() {
                return corner.x * corner.x;
            }
        }
        Map<String, Integer> map = new HashMap<String, Integer>() {
            Entry<String, Integer> first() {
                for (Entry<String, Integer> entry : entrySet()) {
                    return entry;
                }
                return null;
            }

            int total() {
                int sum = 0;
                for (Integer value : values()) {
                    sum += value;
                }
                return sum;
            }
        };
        map.put("a", 1);
        return new Square().area() + map.size();
    }

    List<String> anonymousList(final String[] items) {
        return new AbstractList<String>() {
            @Override
            public String get(int index) {
                return items[index];
            }

            @Override
            public int size() {
                return items.length;
            }
        };
    }

    int diamond(List<String> names) {
        Comparator<String> byLength = new Comparator<>() {
            @Override
            public int compare(String a, String b) {
                return Integer.compare(a.length(), b.length());
            }
        };
        List<String> sorted = new ArrayList<>(names);
        sorted.sort(byLength);
        return sorted.size();
    }

    public static void main(String[] args) {
        LocalClasses demo = new LocalClasses();
        System.out.println(demo.counter(1) + demo.directMember() + demo.localClass(2));
        System.out.println(demo.generic("a", "b") + demo.kinds() + demo.inheritedTypes());
        System.out.println(demo.anonymousList(args).size() + demo.diamond(List.of("x")));
    }
}
