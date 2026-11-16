//- /app/AbstractShape.java
package app;

public abstract class AbstractShape {}
//- /app/Repository.java
package app;

public interface Repository<T, ID> {}
//- /app/Circle.java
package app;

public class Circle extends /*@extends*/ {}
//- /app/CircleRepository.java
package app;

public interface CircleRepository extends Repository</*@type_arg*/, Integer> {}
//? complete extends first AbstractShape
//? complete type_arg has Circle
