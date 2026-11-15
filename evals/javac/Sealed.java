import java.util.List;

public class Sealed {
    sealed interface Shape permits Circle, Square, Rect {}
    record Circle(double radius) implements Shape {}
    record Square(double side) implements Shape {}
    record Rect(double width, double height) implements Shape {}

    public double area(Shape shape) {
        return switch (shape) {
            case Circle c -> Math.PI * c.radius() * c.radius();
            case Square s -> s.side() * s.side();
            case Rect(double w, double h) -> w * h;
        };
    }

    public String classify(Object value) {
        String label;
        if (value instanceof Integer number && number > 10) {
            label = "big" + number;
        } else if (value instanceof String text) {
            label = text.trim();
        } else {
            label = "other";
        }
        return label;
    }

    public int weight(Object value) {
        int result = switch (value) {
            case null -> 0;
            case String s when s.isEmpty() -> 1;
            case String s -> s.length();
            case Circle c -> (int) c.radius();
            default -> -1;
        };
        return result;
    }

    public double sum(List<Shape> shapes) {
        double total = 0;
        for (Shape shape : shapes) {
            double one = area(shape);
            total += one;
        }
        return total;
    }

    public String day(int n) {
        String name = switch (n) {
            case 1, 7 -> "weekend";
            case 2 -> {
                String prefix = "mon";
                yield prefix + "day";
            }
            default -> "weekday";
        };
        return name;
    }
}
