//! The coders of a missile in flight: its profile, motion, cruise schedule and
//! guidance (docs/formats/checkpoint.md). `Flight` holds the seeker, the
//! launch origin and the last intercept the midcourse update steers by; every
//! field is read by a later tick, so every field is coded. The weapon's own
//! record is not here: a projectile carries it as a shared record.

use super::{
    Cruise, FiringBand, Flight, Guidance, LaunchMode, Motion, Profile, Solution, TargetRole,
};

crate::checkpoint_enum!(LaunchMode {
    Cued = 0,
    Boresight = 1,
});

crate::checkpoint_enum!(Guidance {
    Supported = 0,
    Active = 1,
    Infrared = 2,
    Emitter = 3,
    Laser = 4,
    Coordinate = 5,
    Command = 6,
});

crate::checkpoint_enum!(TargetRole {
    Aircraft = 0,
    Surface = 1,
});

crate::checkpoint_struct!(Profile {
    role,
    guidance,
    activation_ft,
    guidance_ticks,
    memory_ticks,
    radar_emissions,
    jammer_emissions,
});

crate::checkpoint_struct!(Cruise { stages, lob });

crate::checkpoint_struct!(Motion {
    velocity,
    gain,
    budget,
    sink,
    sags,
    cruise,
    holds_path,
    energy,
});

crate::checkpoint_struct!(Solution { point, seconds });

crate::checkpoint_struct!(FiringBand { minimum, maximum });

crate::checkpoint_struct!(Flight {
    unguided,
    launch_origin,
    qualified_target,
    profile,
    mode,
    seeker,
    enabled,
    last_intercept,
    solution,
});

#[cfg(test)]
mod tests {
    use super::*;
    use crate::checkpoint::{Models, from_bytes, round_trip, to_bytes};
    use crate::combat::missiles::seeker::{Observation, Seeker, Status};

    fn profile(guidance: Guidance, role: TargetRole) -> Profile {
        Profile {
            role,
            guidance,
            activation_ft: (guidance == Guidance::Active).then_some(5. * super::super::NMI),
            guidance_ticks: 4_800,
            memory_ticks: 240,
            radar_emissions: guidance == Guidance::Emitter,
            jammer_emissions: false,
        }
    }

    #[test]
    fn profiles_cruise_and_motion_round_trip() {
        let models = Models::default();
        for guidance in [
            Guidance::Supported,
            Guidance::Active,
            Guidance::Infrared,
            Guidance::Emitter,
        ] {
            for role in [TargetRole::Aircraft, TargetRole::Surface] {
                let profile = profile(guidance, role);
                assert_eq!(round_trip(&profile, &models).unwrap(), profile);
            }
        }
        let cruise = Cruise {
            stages: [(40_000., 3_000.), (10_000., 500.)],
            lob: true,
        };
        assert_eq!(round_trip(&cruise, &models).unwrap(), cruise);
        let motion = Motion {
            velocity: [12.5, -3.25, 1_800.],
            gain: 410.5,
            budget: 1_200.,
            sink: 17.28,
            sags: true,
            cruise: Some(cruise),
            holds_path: true,
            energy: None,
        };
        assert_eq!(round_trip(&motion, &models).unwrap(), motion);
        let plain = Motion {
            cruise: None,
            sags: false,
            holds_path: false,
            ..motion
        };
        assert_eq!(round_trip(&plain, &models).unwrap(), plain);
        let band = FiringBand {
            minimum: 1_500.,
            maximum: 60_000.,
        };
        assert_eq!(round_trip(&band, &models).unwrap(), band);
    }

    #[test]
    fn a_guided_flight_round_trips_and_a_cut_one_is_refused() {
        let models = Models::default();
        let observation = Observation {
            id: 4,
            position: [500., 10_000., 60_000.],
            velocity: [0., 0., -600.],
            quality: 0.42,
            off_axis: 0.01,
            range: 12_000.,
        };
        let mut flight = Flight::from_supported_launch(
            profile(Guidance::Active, TargetRole::Aircraft),
            LaunchMode::Cued,
            observation,
            [0., 9_000., 0.],
        );
        flight.seeker = Seeker {
            acquired: true,
            status: Status::Pitbull,
            quality: 0.42,
            observation: Some(observation),
            dwell: 30,
            ..flight.seeker
        };
        flight.solution = Some(Solution {
            point: [1., 2., 3.],
            seconds: 14.75,
        });
        let copy = round_trip(&flight, &models).unwrap();
        assert_eq!(copy, flight);
        let unguided = Flight {
            unguided: true,
            ..Flight::new(
                profile(Guidance::Infrared, TargetRole::Aircraft),
                LaunchMode::Boresight,
                None,
                [1., 2., 3.],
            )
        };
        assert_eq!(round_trip(&unguided, &models).unwrap(), unguided);

        let coded = to_bytes(&flight, &models).unwrap();
        for cut in 0..coded.body.len() {
            let mut short = coded.clone();
            short.body.truncate(cut);
            assert!(from_bytes::<Flight>(&short, &models).is_err(), "cut {cut}");
        }
    }
}
