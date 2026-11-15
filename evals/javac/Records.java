import java.util.List;
import java.util.Objects;

public class Records {
    record Point(int x, int y) {
        Point {
            if (x < 0) {
                throw new IllegalArgumentException("x");
            }
        }

        static Point origin() {
            return new Point(0, 0);
        }

        int manhattan(Point other) {
            int dx = Math.abs(x - other.x);
            int dy = Math.abs(y - other.y());
            return dx + dy;
        }
    }

    record Named<T>(String name, T value) {
        Named<T> rename(String next) {
            String checked = Objects.requireNonNull(next);
            return new Named<>(checked, value);
        }
    }

    public int total(List<Point> points) {
        int sum = 0;
        Point origin = Point.origin();
        for (Point p : points) {
            int distance = p.manhattan(origin);
            sum += distance;
        }
        return sum;
    }

    public String describe(Named<Integer> named) {
        Named<Integer> renamed = named.rename("n");
        int doubled = renamed.value() * 2;
        String text = renamed.name() + doubled;
        return text;
    }

    public boolean same(Point a, Point b) {
        boolean equal = a.equals(b);
        int hash = a.hashCode();
        String shown = a.toString();
        return equal && hash == b.hashCode() && shown.isEmpty();
    }
}
