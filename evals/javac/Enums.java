import java.util.EnumMap;
import java.util.Map;

public class Enums {
    enum Planet {
        MERCURY(3.3e23, 2.44e6),
        EARTH(5.97e24, 6.37e6);

        private final double mass;
        private final double radius;

        Planet(double mass, double radius) {
            this.mass = mass;
            this.radius = radius;
        }

        double gravity() {
            double g = 6.67e-11 * mass / (radius * radius);
            return g;
        }
    }

    enum Op {
        ADD {
            @Override
            int apply(int a, int b) {
                return a + b;
            }
        },
        MUL {
            @Override
            int apply(int a, int b) {
                return a * b;
            }
        };

        abstract int apply(int a, int b);
    }

    public double heaviest() {
        double best = 0;
        for (Planet planet : Planet.values()) {
            double gravity = planet.gravity();
            if (gravity > best) {
                best = gravity;
            }
        }
        return best;
    }

    public int compute(Op op, int a, int b) {
        int result = op.apply(a, b);
        return result;
    }

    public String name(Planet planet) {
        switch (planet) {
            case MERCURY:
                return "mercury";
            case EARTH:
                return "earth";
            default:
                throw new IllegalArgumentException();
        }
    }

    public Map<Planet, String> names() {
        Map<Planet, String> map = new EnumMap<>(Planet.class);
        for (Planet planet : Planet.values()) {
            String lower = planet.name().toLowerCase();
            map.put(planet, lower);
        }
        return map;
    }

    public int ordinalSum() {
        Planet earth = Planet.valueOf("EARTH");
        int ordinal = earth.ordinal();
        int compared = earth.compareTo(Planet.MERCURY);
        return ordinal + compared;
    }
}
