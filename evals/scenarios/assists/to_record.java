import java.util.List;
import java.util.Objects;

final class /*@point*/Point {
    private final int x;
    private final int y;

    public Point(int x, int y) {
        this.x = x;
        this.y = y;
    }

    public int x() {
        return x;
    }

    public int getY() {
        return y;
    }
}

class /*@range*/Range<T extends Comparable<T>> implements Comparable<Range<T>> {
    private final T low;
    private final T high;

    Range(T low, T high) {
        Objects.requireNonNull(low);
        if (low.compareTo(high) > 0) {
            throw new IllegalArgumentException();
        }
        this.low = low;
        this.high = high;
    }

    Range(T only) {
        this(only, only);
    }

    public int compareTo(Range<T> other) {
        return low.compareTo(other.low);
    }
}

class /*@swapped*/Swapped {
    private final String name;
    private final List<String> tags;

    Swapped(List<String> tags, String name) {
        this.name = name;
        this.tags = tags;
    }
}

class /*@mutable*/Mutable {
    private int count;

    Mutable(int count) {
        this.count = count;
    }
}

class /*@base*/Base {
    private final int id;

    Base(int id) {
        this.id = id;
    }
}

class Derived extends Base {
    Derived() {
        super(1);
    }
}

class /*@factory*/Factory {
    private final int id;

    private Factory(int id) {
        this.id = id;
    }

    static Factory of(int id) {
        return new Factory(id);
    }
}

class /*@hidden*/Hidden {
    private final int size;

    Hidden(int size) {
        this.size = size;
    }

    int size() {
        return size * 2;
    }
}

class Outer {
    class /*@inner*/Inner {
        private final int v;

        Inner(int v) {
            this.v = v;
        }
    }
}
//? assist convert_to_record point ; has "record Point(int x, int y) {\n    public int getY() {\n        return y;\n    }\n}" ; lacks "final class" ; lacks "this.x = x"
//? assist convert_to_record range ; has "record Range<T extends Comparable<T>>(T low, T high) implements Comparable<Range<T>> {\n    Range {\n        Objects.requireNonNull(low);\n        if (low.compareTo(high) > 0) {\n            throw new IllegalArgumentException();\n        }\n    }\n\n    Range(T only) {\n        this(only, only);\n    }" ; lacks "this.low = low"
//? assist convert_to_record swapped ; has "record Swapped(List<String> tags, String name) {\n}"
//? noassist convert_to_record mutable
//? noassist convert_to_record base
//? noassist convert_to_record factory
//? noassist convert_to_record hidden
//? noassist convert_to_record inner
